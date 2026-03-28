use crate::common::{
    JavaPlaySession, PreparedServer, StatusSession, assert_upgrade_task_succeeded,
    fetch_runtime_tcp_listener_addr, reload_runtime_full, remote_admin_upgrade_options,
    shutdown_runtime_via_grpc, spawn_upgrade_task, upgrade_to_current_bootstrap,
    wait_for_clean_parent_exit, wait_for_upgrade_phase,
};
use crate::support::{
    PersistedServerLogCapture, ProcessTestClientEncryptionState, TestResult, begin_online_login,
    complete_online_login, held_item_change, held_item_from_packet, packet_id,
    prepare_online_auth_runtime_plugins, read_until_java_packet_encrypted, write_packet_encrypted,
};
use bytes::BytesMut;
use mc_plugin_admin_grpc::admin as proto;
use mc_proto_common::MinecraftWireCodec;
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use rsa::RsaPublicKey;
use std::net::{SocketAddr, TcpStream};
use std::process::Child;
use std::time::Duration;
use tonic::Code;

pub(crate) type AdminClient = crate::common::AdminClient;
pub(crate) type UpgradeTask = crate::common::UpgradeTask;

pub(crate) struct LoggedRuntime {
    pub(crate) server: PreparedServer,
    pub(crate) child: Child,
    pub(crate) logs: PersistedServerLogCapture,
    client: Option<AdminClient>,
    pub(crate) game_addr: SocketAddr,
}

pub(crate) struct PendingOnlineLogin {
    stream: TcpStream,
    buffer: BytesMut,
    public_key: RsaPublicKey,
    verify_token: Vec<u8>,
}

pub(crate) struct FreezeHarness {
    pub(crate) runtime: LoggedRuntime,
    upgrade_client: Option<AdminClient>,
    status_client: AdminClient,
    reload_client: AdminClient,
    shutdown_client: AdminClient,
    second_upgrade_client: AdminClient,
    session: JavaPlaySession,
}

pub(crate) async fn start_logged_runtime(
    server: PreparedServer,
    capture_name: &'static str,
    timeout: Duration,
) -> TestResult<LoggedRuntime> {
    let (child, logs) = server.spawn_logged(capture_name)?;
    let mut client = server.wait_for_client(timeout).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
    Ok(LoggedRuntime {
        server,
        child,
        logs,
        client: Some(client),
        game_addr,
    })
}

fn runtime_client(runtime: &mut LoggedRuntime) -> TestResult<&mut AdminClient> {
    runtime
        .client
        .as_mut()
        .ok_or("runtime gRPC client was unavailable".into())
}

pub(crate) async fn cutover_logged_runtime(
    runtime: &mut LoggedRuntime,
    timeout: Duration,
) -> TestResult<AdminClient> {
    upgrade_to_current_bootstrap(runtime_client(runtime)?).await?;
    wait_for_child_client(runtime, timeout).await
}

pub(crate) async fn shutdown_logged_runtime(
    runtime: &mut LoggedRuntime,
    child_client: &mut AdminClient,
) -> TestResult<()> {
    shutdown_runtime_via_grpc(
        child_client,
        runtime.server.grpc_addr,
        &mut runtime.child,
        &runtime.logs,
    )
    .await
}

async fn wait_for_child_client(
    runtime: &LoggedRuntime,
    timeout: Duration,
) -> TestResult<AdminClient> {
    let mut child_client = runtime.server.wait_for_client(timeout).await?;
    assert_eq!(
        fetch_runtime_tcp_listener_addr(&mut child_client).await?,
        runtime.game_addr
    );
    Ok(child_client)
}

pub(crate) async fn start_play_runtime(
    motd: &'static str,
    capture_name: &'static str,
    username: &str,
    timeout: Duration,
    login_context: &str,
    bootstrap_context: &str,
) -> TestResult<(LoggedRuntime, JavaPlaySession)> {
    let runtime =
        start_logged_runtime(PreparedServer::remote_admin(motd)?, capture_name, timeout).await?;
    let mut session = JavaPlaySession::connect(runtime.game_addr, username, login_context)?;
    session.wait_for_bootstrap(bootstrap_context)?;
    Ok((runtime, session))
}

pub(crate) async fn start_reloaded_play_runtime() -> TestResult<(LoggedRuntime, JavaPlaySession)> {
    let runtime = start_reloaded_runtime().await?;
    let session = connect_reloaded_play_session(runtime.game_addr)?;
    Ok((runtime, session))
}

async fn start_reloaded_runtime() -> TestResult<LoggedRuntime> {
    let mut runtime = start_logged_runtime(
        PreparedServer::remote_admin("grpc-upgrade-after-full-reload")?,
        "grpc-upgrade-after-full-reload",
        Duration::from_secs(5),
    )
    .await?;
    reload_runtime_full(runtime_client(&mut runtime)?).await?;
    runtime.game_addr = fetch_runtime_tcp_listener_addr(runtime_client(&mut runtime)?).await?;
    Ok(runtime)
}

fn connect_reloaded_play_session(game_addr: SocketAddr) -> TestResult<JavaPlaySession> {
    let mut session =
        JavaPlaySession::connect(game_addr, "up-reload-a", "post-reload play login failed")?;
    session.wait_for_bootstrap("post-reload held-item bootstrap failed")?;
    Ok(session)
}

pub(crate) fn assert_play_roundtrip(
    session: &mut JavaPlaySession,
    slot: i16,
    write_context: &str,
    read_context: &str,
) -> TestResult<()> {
    session.assert_held_item_roundtrip(slot, write_context, read_context)
}

pub(crate) fn assert_post_cutover_play_roundtrip(
    session: &mut JavaPlaySession,
    slot: i16,
    missing_packet_context: &str,
    write_context: &str,
    read_context: &str,
) -> TestResult<()> {
    session.assert_no_packet(
        TestJavaPacket::LoginSuccess,
        Duration::from_millis(400),
        missing_packet_context,
    )?;
    assert_play_roundtrip(session, slot, write_context, read_context)
}

pub(crate) fn assert_new_player_can_join(
    game_addr: SocketAddr,
    username: &str,
    context: &str,
) -> TestResult<()> {
    JavaPlaySession::connect_additional_player(game_addr, username, context)
}

pub(crate) async fn start_freeze_harness() -> TestResult<FreezeHarness> {
    let (
        runtime,
        upgrade_client,
        status_client,
        reload_client,
        shutdown_client,
        second_upgrade_client,
    ) = spawn_freeze_runtime().await?;
    let mut session = start_freeze_session(runtime.game_addr)?;
    session.wait_for_bootstrap("freeze bootstrap held-item read failed")?;
    Ok(FreezeHarness {
        runtime,
        upgrade_client: Some(upgrade_client),
        status_client,
        reload_client,
        shutdown_client,
        second_upgrade_client,
        session,
    })
}

pub(crate) fn spawn_freeze_upgrade(harness: &mut FreezeHarness) -> TestResult<UpgradeTask> {
    let client = harness
        .upgrade_client
        .take()
        .ok_or("freeze upgrade client was already consumed")?;
    Ok(spawn_upgrade_task(client))
}

pub(crate) async fn wait_for_parent_freeze(harness: &mut FreezeHarness) -> TestResult<()> {
    let freeze_status = wait_for_upgrade_phase(
        &mut harness.status_client,
        proto::RuntimeUpgradeRole::Parent,
        proto::RuntimeUpgradePhase::ParentFreezing,
        Duration::from_secs(5),
    )
    .await?;
    assert!(freeze_status.upgrade.is_some());
    Ok(())
}

pub(crate) async fn assert_freeze_mutations_rejected(
    harness: &mut FreezeHarness,
) -> TestResult<()> {
    let reload_error = harness
        .reload_client
        .reload_runtime(crate::common::authorized_request(
            proto::ReloadRuntimeRequest {
                mode: proto::RuntimeReloadMode::Full as i32,
            },
        ))
        .await
        .expect_err("reload should be rejected while upgrade freeze is active");
    assert_eq!(reload_error.code(), Code::FailedPrecondition);

    let shutdown_error = harness
        .shutdown_client
        .shutdown(crate::common::authorized_request(proto::ShutdownRequest {}))
        .await
        .expect_err("shutdown should be rejected while upgrade freeze is active");
    assert_eq!(shutdown_error.code(), Code::FailedPrecondition);

    let second_upgrade_error = harness
        .second_upgrade_client
        .upgrade_runtime(crate::common::authorized_request(
            proto::UpgradeRuntimeRequest {
                executable_path: crate::common::SERVER_BOOTSTRAP_BIN.to_string(),
            },
        ))
        .await
        .expect_err("second upgrade should be rejected while upgrade freeze is active");
    assert_eq!(second_upgrade_error.code(), Code::FailedPrecondition);
    Ok(())
}

pub(crate) fn assert_freeze_buffers_play_packet(harness: &mut FreezeHarness) -> TestResult<()> {
    harness
        .session
        .set_held_item(8, "freeze held-item write failed")?;
    harness.session.assert_no_packet(
        TestJavaPacket::HeldItemChange,
        Duration::from_millis(200),
        "held-item change should stay buffered during freeze",
    )
}

pub(crate) async fn finish_freeze_scenario(
    harness: &mut FreezeHarness,
    upgrade_task: UpgradeTask,
) -> TestResult<()> {
    assert_upgrade_task_succeeded(upgrade_task).await?;
    let mut child_client = wait_for_child_client(&harness.runtime, Duration::from_secs(5)).await?;
    assert_eq!(
        harness
            .session
            .read_held_item("buffered held-item change was not delivered after cutover")?,
        8
    );
    shutdown_logged_runtime(&mut harness.runtime, &mut child_client).await
}

pub(crate) async fn start_status_runtime() -> TestResult<(LoggedRuntime, StatusSession)> {
    let runtime = start_logged_runtime(
        PreparedServer::remote_admin("grpc-upgrade-status-continuity")?,
        "grpc-upgrade-status-continuity",
        Duration::from_secs(5),
    )
    .await?;
    let status_session =
        StatusSession::connect(runtime.game_addr, "grpc-upgrade-status-continuity")?;
    Ok((runtime, status_session))
}

pub(crate) fn assert_status_session_survives(
    runtime: &mut LoggedRuntime,
    status_session: &mut StatusSession,
) -> TestResult<()> {
    wait_for_clean_parent_exit(
        &mut runtime.child,
        &runtime.logs,
        Duration::from_secs(5),
        "original parent process should exit after successful cutover even while the pre-cutover status session stays open",
    )?;
    assert_eq!(status_session.ping(12_345)?, 12_345);
    Ok(())
}

pub(crate) fn build_online_runtime() -> TestResult<PreparedServer> {
    PreparedServer::new(|temp_path, grpc_port| {
        let runtime_plugins_dir =
            prepare_online_auth_runtime_plugins(temp_path, "grpc-upgrade-online-login")?;
        let mut options = remote_admin_upgrade_options(grpc_port, "grpc-upgrade-online-login");
        options.online_mode = true;
        options.auth_profile = "mojang-online-v1";
        options.extra_plugin_allowlist = &["auth-online-stub"];
        options.plugins_dir_override = Some(runtime_plugins_dir);
        Ok(options)
    })
}

pub(crate) fn begin_pending_online_login(
    game_addr: SocketAddr,
    username: &str,
    context: &str,
) -> TestResult<PendingOnlineLogin> {
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let mut stream = crate::support::connect_tcp(game_addr)?;
    let (buffer, public_key, verify_token) =
        begin_online_login(&mut stream, &codec, protocol, username)
            .map_err(|error| format!("{context}: {error}"))?;
    Ok(PendingOnlineLogin {
        stream,
        buffer,
        public_key,
        verify_token,
    })
}

pub(crate) fn complete_cutover_online_login(
    pending: &mut PendingOnlineLogin,
) -> TestResult<ProcessTestClientEncryptionState> {
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let mut encryption = complete_online_login(
        &mut pending.stream,
        &codec,
        &pending.public_key,
        &pending.verify_token,
    )
    .map_err(|error| format!("post-upgrade online login completion failed: {error}"))?;
    let login_success = read_until_java_packet_encrypted(
        &mut pending.stream,
        &codec,
        &mut pending.buffer,
        protocol,
        TestJavaPacket::LoginSuccess,
        Duration::from_secs(5),
        12,
        &mut encryption,
    )
    .map_err(|error| format!("encrypted login success after cutover failed: {error}"))?;
    assert_eq!(packet_id(&login_success)?, 0x02);
    Ok(encryption)
}

pub(crate) fn assert_online_bootstrap_and_roundtrip(
    pending: &mut PendingOnlineLogin,
    encryption: &mut ProcessTestClientEncryptionState,
) -> TestResult<()> {
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let _ = read_until_java_packet_encrypted(
        &mut pending.stream,
        &codec,
        &mut pending.buffer,
        protocol,
        TestJavaPacket::WindowItems,
        Duration::from_secs(5),
        24,
        encryption,
    )
    .map_err(|error| format!("encrypted window-items bootstrap after cutover failed: {error}"))?;
    let _ = read_until_java_packet_encrypted(
        &mut pending.stream,
        &codec,
        &mut pending.buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
        encryption,
    )
    .map_err(|error| format!("encrypted held-item bootstrap after cutover failed: {error}"))?;
    write_packet_encrypted(
        &mut pending.stream,
        &codec,
        &held_item_change(5),
        encryption,
    )?;
    let changed_slot = read_until_java_packet_encrypted(
        &mut pending.stream,
        &codec,
        &mut pending.buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        12,
        encryption,
    )
    .map_err(|error| format!("encrypted held-item echo after cutover failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &changed_slot)?, 5);
    Ok(())
}

async fn spawn_freeze_runtime() -> TestResult<(
    LoggedRuntime,
    AdminClient,
    AdminClient,
    AdminClient,
    AdminClient,
    AdminClient,
)> {
    let server = PreparedServer::remote_admin("grpc-upgrade-freeze-phase")?;
    let (child, logs) = server.spawn_logged_with_envs(
        "grpc-upgrade-freeze-phase",
        &[("REVY_UPGRADE_TEST_HOLD_AFTER_SESSION_FREEZE_MS", "600")],
    )?;
    let (mut upgrade_client, status_client, reload_client, shutdown_client, second_upgrade_client) =
        connect_freeze_admin_clients(&server).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut upgrade_client).await?;
    Ok((
        LoggedRuntime {
            server,
            child,
            logs,
            client: None,
            game_addr,
        },
        upgrade_client,
        status_client,
        reload_client,
        shutdown_client,
        second_upgrade_client,
    ))
}

async fn connect_freeze_admin_clients(
    server: &PreparedServer,
) -> TestResult<(
    AdminClient,
    AdminClient,
    AdminClient,
    AdminClient,
    AdminClient,
)> {
    Ok((
        server.wait_for_client(Duration::from_secs(5)).await?,
        server.wait_for_client(Duration::from_secs(5)).await?,
        server.wait_for_client(Duration::from_secs(5)).await?,
        server.wait_for_client(Duration::from_secs(5)).await?,
        server.wait_for_client(Duration::from_secs(5)).await?,
    ))
}

fn start_freeze_session(game_addr: SocketAddr) -> TestResult<JavaPlaySession> {
    JavaPlaySession::connect(game_addr, "freeze-play-a", "freeze test login failed")
}
