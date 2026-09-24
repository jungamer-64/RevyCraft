use crate::bedrock_oracle::BedrockOracle;
use crate::common::{
    PreparedServer, SERVER_BOOTSTRAP_BIN, fetch_runtime_tcp_listener_addr, fetch_status,
    persisted_log_diagnostics, reload_runtime, remote_admin_upgrade_options,
    runtime_udp_listener_addr, shutdown_runtime_via_grpc, upgrade_runtime_executable,
};
use crate::support::{
    TestResult, encode_handshake, held_item_change, held_item_from_packet, login_start, packet_id,
};
use bytes::BytesMut;
use mc_plugin_admin_grpc::admin as proto;
use mc_proto_common::{MinecraftWireCodec, PacketReader, PacketWriter, WireCodec};
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

const LATENCY_SESSION_COUNT: usize = 1_000;
const LATENCY_VIEW_GROUP_COUNT: usize = 100;
const LATENCY_COMMAND_ATTEMPT_LIMIT: u8 = 16;
const LATENCY_COMMAND_RESPONSE_WINDOW: Duration = Duration::from_millis(250);
const LATENCY_WORKLOAD_PERIOD: Duration = Duration::from_millis(20);
const JAVA_CONNECT_FAN_OUT: usize = 32;
const BEDROCK_STABILIZATION_FAN_OUT: usize = 128;
const BEDROCK_CONNECT_FAN_OUT: usize = 8;
const BEDROCK_SESSION_CONNECT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Copy)]
enum LatencyOperation {
    Reload(proto::RuntimeReloadMode),
    ExecutableUpgrade,
}

#[derive(Serialize)]
struct LatencyArtifact {
    schema_version: u32,
    operation: &'static str,
    connection_mix: LatencyConnectionMix,
    warm_up_count: usize,
    samples: Vec<LatencySample>,
}

#[derive(Clone, Serialize)]
struct LatencyConnectionMix {
    java: u64,
    bedrock: u64,
}

#[derive(Clone, Serialize)]
struct LatencySample {
    operation: &'static str,
    mode: Option<&'static str>,
    session_count: u64,
    connection_mix: LatencyConnectionMix,
    stage_us: u64,
    prepare_us: u64,
    freeze_us: u64,
    resume_us: u64,
    outcome: &'static str,
    epoch_revision: u64,
}

struct LatencyProcessGuard {
    process: Option<Child>,
}

struct JavaLatencySession {
    command_tx: mpsc::Sender<JavaLatencyCommand>,
    terminal_error: watch::Receiver<Option<String>>,
    tasks: JoinSet<()>,
}

struct BedrockLatencySession {
    command_tx: mpsc::Sender<BedrockLatencyWorkload>,
    tasks: JoinSet<()>,
}

enum BedrockLatencyWorkload {
    Stabilize {
        slot: i8,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    CompleteGameplay {
        slot: i8,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ArmInFlight {
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ReleaseCutover {
        response_tx: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Clone, Copy)]
struct LatencyMix {
    java: usize,
    bedrock: usize,
    name: &'static str,
}

struct JavaLatencyCommand {
    slot: i16,
    attempts: u8,
    response_tx: oneshot::Sender<Result<(), String>>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "the cutover latency workflow supplies the 1000-session scenario and sample tier"]
async fn production_cutover_latency_java_1000() -> TestResult<()> {
    run_production_cutover_latency(LatencyMix {
        java: LATENCY_SESSION_COUNT,
        bedrock: 0,
        name: "java-1000",
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "the cutover latency workflow supplies the 1000-session scenario and sample tier"]
async fn production_cutover_latency_bedrock_1000() -> TestResult<()> {
    run_production_cutover_latency(LatencyMix {
        java: 0,
        bedrock: LATENCY_SESSION_COUNT,
        name: "bedrock-1000",
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "the cutover latency workflow supplies the 1000-session scenario and sample tier"]
async fn production_cutover_latency_mixed_500_500() -> TestResult<()> {
    run_production_cutover_latency(LatencyMix {
        java: LATENCY_SESSION_COUNT / 2,
        bedrock: LATENCY_SESSION_COUNT / 2,
        name: "mixed-500-500",
    })
    .await
}

async fn run_production_cutover_latency(mix: LatencyMix) -> TestResult<()> {
    let operation = latency_operation()?;
    let (warm_up_count, measured_count) = latency_tier(operation)?;
    if operation.is_scoped_reload() && (mix.java != 500 || mix.bedrock != 500) {
        return Err("scoped reload latency requires the mixed 1000-session population".into());
    }
    let artifact_path = required_path("REVY_CUTOVER_LATENCY_ARTIFACT")?;
    let scenario_name = format!("cutover-latency-{}", mix.name);
    let server = PreparedServer::new(|_, grpc_port| {
        let mut options = remote_admin_upgrade_options(grpc_port, "cutover-latency");
        options.max_players = LATENCY_SESSION_COUNT;
        options.view_distance = 1;
        options.bedrock_enabled = mix.bedrock != 0;
        Ok(options)
    })?;
    let (process, logs) = server.spawn_logged(&scenario_name)?;
    let mut process = LatencyProcessGuard::new(process);
    let mut client = match server.wait_for_client(Duration::from_secs(15)).await {
        Ok(client) => client,
        Err(error) => {
            return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
        }
    };
    // The operator shuts down the live population. Retain clients through that boundary:
    // dropping them after the final sample would initiate a separate mass-disconnect workload.
    let mut java_sessions = Vec::new();
    let mut bedrock_sessions = Vec::new();
    let workload_result: TestResult<()> = async {
        let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
        java_sessions = match connect_java_sessions(game_addr, mix.java).await {
            Ok(sessions) => sessions,
            Err(error) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
            }
        };
        eprintln!(
            "latency workload: {} Java sessions ready",
            java_sessions.len()
        );
        bedrock_sessions = if mix.bedrock == 0 {
            Vec::new()
        } else {
            let status = fetch_status(&mut client).await?;
            let udp_addr = runtime_udp_listener_addr(&status)?;
            match connect_bedrock_sessions(udp_addr, mix.bedrock).await {
                Ok(sessions) => sessions,
                Err(error) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
                }
            }
        };
        eprintln!(
            "latency workload: {} Bedrock sessions ready",
            bedrock_sessions.len()
        );
        assert_latency_sessions_ready(&mut client, mix).await?;
        run_java_roundtrip(&java_sessions, 7, Duration::from_secs(30)).await?;
        let stabilization = run_bedrock_roundtrip(
            &bedrock_sessions,
            7,
            Duration::from_secs(120),
            BedrockWorkloadKind::Stabilize,
        )
        .await;
        if let Err(error) = stabilization {
            tokio::time::sleep(Duration::from_secs(1)).await;
            return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
        }
        eprintln!("latency workload: session stabilization complete");
        let mut samples = Vec::with_capacity(measured_count);

        for iteration in 0..warm_up_count + measured_count {
            eprintln!(
                "latency workload: starting cutover sample {}/{}",
                iteration + 1,
                warm_up_count + measured_count
            );
            let java_workload = run_java_steady_workload(&java_sessions, iteration);
            let bedrock_workload = run_bedrock_steady_workload(&bedrock_sessions, iteration);
            if let Err(error) = tokio::try_join!(java_workload, bedrock_workload) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
            }
            if let Err(error) = arm_bedrock_cutover(&bedrock_sessions).await {
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
            }
            // Keep real gameplay flowing throughout staging and prepare, not just before the
            // operator request. The command future must finish even if the workload fails:
            // cancelling an upgrade request would leave its commit outcome uncertain.
            let (stop_tx, stop_rx) = watch::channel(false);
            let workload = run_continuous_workload(
                &java_sessions,
                &bedrock_sessions,
                iteration + 1,
                stop_rx,
            );
            let cutover = async {
                let result: TestResult<proto::CutoverReport> = async {
                    match operation {
                        LatencyOperation::Reload(mode) => reload_runtime(&mut client, mode).await,
                        LatencyOperation::ExecutableUpgrade => {
                            let result = upgrade_runtime_executable(
                                &mut client,
                                SERVER_BOOTSTRAP_BIN,
                            ).await?;
                            let report = result.cutover
                                .ok_or("upgrade response was missing cutover report")?;
                            client = server.wait_for_client(Duration::from_secs(15)).await?;
                            Ok(report)
                        }
                    }
                }.await;
                stop_tx.send_replace(true);
                result
            };
            tokio::pin!(cutover, workload);
            let mut completed_workload = None;
            let cutover_result = tokio::select! {
                result = &mut cutover => result,
                result = &mut workload => {
                    completed_workload = Some(result);
                    (&mut cutover).await
                }
            };
            let report = cutover_result?;
            let report_mix = report
                .connection_mix
                .as_ref()
                .ok_or("latency report omitted connection mix")?;
            if report_mix.java != u64::try_from(mix.java)?
                || report_mix.bedrock != u64::try_from(mix.bedrock)?
            {
                return Err(format!(
                    "latency report connection mix was Java {} + Bedrock {}, expected Java {} + Bedrock {}",
                    report_mix.java, report_mix.bedrock, mix.java, mix.bedrock
                )
                .into());
            }
            if iteration >= warm_up_count {
                eprintln!(
                    "latency workload: cutover freeze={}us resume={}us",
                    report.freeze_us, report.resume_us
                );
                let sample = latency_sample(report)?;
                let freeze_us = sample.freeze_us;
                samples.push(sample);
                write_latency_artifact(&artifact_path, operation, mix, warm_up_count, &samples)?;
                if freeze_us > operation.hard_limit_us() {
                    return Err(format!(
                        "cutover freeze {freeze_us}us exceeded the {}us CI hard limit",
                        operation.hard_limit_us()
                    )
                    .into());
                }
            }
            // Persist and check the report as soon as the operator returns. A later gameplay
            // timeout must not conceal a completed cutover's hard-limit violation.
            match completed_workload {
                Some(result) => result?,
                None => workload.await?,
            }
            if let Err(error) = release_bedrock_cutover(&bedrock_sessions).await {
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Err(format!("{error}; {}", persisted_log_diagnostics(&logs)).into());
            }
        }

        // Intermediate generations are exercised by the next iteration. The final generation
        // must also serve gameplay on every original connection before the workload can pass.
        let final_iteration = warm_up_count + measured_count;
        if let Err(error) = tokio::try_join!(
            run_java_steady_workload(&java_sessions, final_iteration),
            run_bedrock_steady_workload(&bedrock_sessions, final_iteration),
        ) {
            return Err(format!("final-generation gameplay failed: {error}; {}", persisted_log_diagnostics(&logs)).into());
        }
        assert_latency_sessions_ready(&mut client, mix).await?;
        eprintln!("latency workload: final generation served all original sessions");
        write_latency_artifact(&artifact_path, operation, mix, warm_up_count, &samples)?;
        Ok(())
    }
    .await;

    if let Ok(active_client) = server.wait_for_client(Duration::from_secs(5)).await {
        client = active_client;
    }
    let shutdown =
        shutdown_runtime_via_grpc(&mut client, server.grpc_addr, process.process_mut(), &logs)
            .await;
    if shutdown.is_ok() {
        process.disarm();
    }
    let clients = finish_latency_clients(java_sessions, bedrock_sessions).await;
    let result = match (workload_result, shutdown) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(shutdown_error)) => {
            Err(format!("{error}; latency runtime cleanup failed: {shutdown_error}").into())
        }
    };
    match (result, clients) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(error), Err(client_error)) => {
            Err(format!("{error}; latency client cleanup failed: {client_error}").into())
        }
    }
}

async fn finish_latency_clients(
    java: Vec<JavaLatencySession>,
    bedrock: Vec<BedrockLatencySession>,
) -> TestResult<()> {
    let mut tasks = java
        .into_iter()
        .map(|session| session.tasks)
        .chain(bedrock.into_iter().map(|session| session.tasks))
        .collect::<Vec<_>>();
    for tasks in &mut tasks {
        tasks.abort_all();
    }
    let mut failure = None;
    for tasks in &mut tasks {
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
    }
    match failure {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn write_latency_artifact(
    artifact_path: &std::path::Path,
    operation: LatencyOperation,
    mix: LatencyMix,
    warm_up_count: usize,
    samples: &[LatencySample],
) -> TestResult<()> {
    let artifact = LatencyArtifact {
        schema_version: 1,
        operation: operation.name(),
        connection_mix: LatencyConnectionMix {
            java: u64::try_from(mix.java)?,
            bedrock: u64::try_from(mix.bedrock)?,
        },
        warm_up_count,
        samples: samples.to_vec(),
    };
    let encoded = serde_json::to_vec_pretty(&artifact)?;
    if let Some(parent) = artifact_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&artifact_path, encoded)?;
    Ok(())
}

async fn assert_latency_sessions_ready(
    client: &mut crate::common::AdminClient,
    mix: LatencyMix,
) -> TestResult<()> {
    let status = fetch_status(client).await?;
    let summary = status
        .session_summary
        .ok_or("latency status omitted session summary")?;
    let expected_total = u64::try_from(mix.java.saturating_add(mix.bedrock))?;
    let play_count = summary
        .by_phase
        .iter()
        .find(|count| count.phase == proto::ConnectionPhase::Play as i32)
        .map_or(0, |count| count.count);
    let java_count = summary
        .by_transport
        .iter()
        .find(|count| count.transport == proto::TransportKind::Tcp as i32)
        .map_or(0, |count| count.count);
    let bedrock_count = summary
        .by_transport
        .iter()
        .find(|count| count.transport == proto::TransportKind::Udp as i32)
        .map_or(0, |count| count.count);
    if summary.total != expected_total
        || play_count != expected_total
        || java_count != u64::try_from(mix.java)?
        || bedrock_count != u64::try_from(mix.bedrock)?
    {
        return Err(format!(
            "latency sessions were not all Play: total={} play={} java={} bedrock={}, expected Java {} + Bedrock {}",
            summary.total, play_count, java_count, bedrock_count, mix.java, mix.bedrock
        )
        .into());
    }
    Ok(())
}

impl LatencyProcessGuard {
    const fn new(process: Child) -> Self {
        Self {
            process: Some(process),
        }
    }

    fn process_mut(&mut self) -> &mut Child {
        self.process
            .as_mut()
            .expect("latency process guard remains armed")
    }

    fn disarm(&mut self) {
        self.process = None;
    }
}

impl Drop for LatencyProcessGuard {
    fn drop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

impl LatencyOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::Reload(_) => "reload",
            Self::ExecutableUpgrade => "executable-upgrade",
        }
    }

    const fn hard_limit_us(self) -> u64 {
        match self {
            Self::Reload(_) => 200_000,
            Self::ExecutableUpgrade => 500_000,
        }
    }

    const fn is_scoped_reload(self) -> bool {
        matches!(self, Self::Reload(mode) if !matches!(mode, proto::RuntimeReloadMode::Full))
    }
}

fn latency_operation() -> TestResult<LatencyOperation> {
    match std::env::var("REVY_CUTOVER_LATENCY_OPERATION").as_deref() {
        Ok("reload") => Ok(LatencyOperation::Reload(proto::RuntimeReloadMode::Full)),
        Ok("reload-artifacts") => Ok(LatencyOperation::Reload(
            proto::RuntimeReloadMode::Artifacts,
        )),
        Ok("reload-topology") => Ok(LatencyOperation::Reload(proto::RuntimeReloadMode::Topology)),
        Ok("reload-core") => Ok(LatencyOperation::Reload(proto::RuntimeReloadMode::Core)),
        Ok("executable-upgrade") => Ok(LatencyOperation::ExecutableUpgrade),
        Ok(value) => Err(format!("unsupported latency operation `{value}`").into()),
        Err(error) => Err(format!("REVY_CUTOVER_LATENCY_OPERATION is required: {error}").into()),
    }
}

fn latency_tier(operation: LatencyOperation) -> TestResult<(usize, usize)> {
    match std::env::var("REVY_CUTOVER_LATENCY_TIER").as_deref() {
        Ok("pull-request") => Ok((1, 5)),
        Ok("acceptance") if operation.is_scoped_reload() => Ok((1, 5)),
        Ok("acceptance") => Ok((3, 20)),
        Ok(value) => Err(format!("unsupported latency tier `{value}`").into()),
        Err(error) => Err(format!("REVY_CUTOVER_LATENCY_TIER is required: {error}").into()),
    }
}

fn required_path(name: &str) -> TestResult<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is required").into())
}

async fn connect_java_sessions(
    addr: std::net::SocketAddr,
    count: usize,
) -> TestResult<Vec<JavaLatencySession>> {
    let mut sessions = Vec::with_capacity(count);
    let mut next_index = 0_usize;
    let mut connecting = tokio::task::JoinSet::new();
    while next_index < count && connecting.len() < JAVA_CONNECT_FAN_OUT {
        spawn_java_connection(&mut connecting, addr, next_index);
        next_index += 1;
    }
    while let Some(result) = connecting.join_next().await {
        sessions.push(result??);
        if sessions.len().is_multiple_of(100) || sessions.len() == count {
            eprintln!(
                "latency workload: {}/{} Java sessions admitted",
                sessions.len(),
                count
            );
        }
        if next_index < count {
            spawn_java_connection(&mut connecting, addr, next_index);
            next_index += 1;
        }
    }
    sessions.sort_by_key(|(index, _)| *index);
    let sessions = sessions.into_iter().map(|(_, session)| session).collect();
    Ok(sessions)
}

fn spawn_java_connection(
    connecting: &mut tokio::task::JoinSet<Result<(usize, JavaLatencySession), String>>,
    addr: std::net::SocketAddr,
    index: usize,
) {
    connecting.spawn(async move {
        let view_group = index % LATENCY_VIEW_GROUP_COUNT;
        let x = (view_group + 1) as f64 * 96.0 + 0.5;
        let session = JavaLatencySession::connect(addr, &format!("p{index:04}"), x)
            .await
            .map_err(|error| {
                format!("failed to establish Java latency session {index}: {error}")
            })?;
        let response = session.begin_roundtrip(8).await.map_err(|error| {
            format!("Java relocation command failed for session {index}: {error}")
        })?;
        tokio::time::timeout(Duration::from_secs(30), response)
            .await
            .map_err(|_| format!("Java relocation timed out for session {index}"))?
            .map_err(|_| format!("Java relocation actor {index} stopped"))?
            .map_err(|error| format!("Java relocation failed for session {index}: {error}"))?;
        Ok((index, session))
    });
}

async fn connect_bedrock_sessions(
    addr: std::net::SocketAddr,
    count: usize,
) -> TestResult<Vec<BedrockLatencySession>> {
    let mut sessions = Vec::with_capacity(count);
    let mut next_index = 0_usize;
    let mut connecting = tokio::task::JoinSet::new();
    while next_index < count && connecting.len() < BEDROCK_CONNECT_FAN_OUT {
        spawn_bedrock_connection(&mut connecting, addr, next_index);
        next_index += 1;
    }
    while let Some(result) = connecting.join_next().await {
        sessions.push(result??);
        if sessions.len().is_multiple_of(100) || sessions.len() == count {
            eprintln!(
                "latency workload: {}/{} Bedrock sessions admitted",
                sessions.len(),
                count
            );
        }
        if next_index < count {
            spawn_bedrock_connection(&mut connecting, addr, next_index);
            next_index += 1;
        }
    }
    sessions.sort_by_key(|(index, _)| *index);
    let sessions = sessions.into_iter().map(|(_, session)| session).collect();
    Ok(sessions)
}

fn spawn_bedrock_connection(
    connecting: &mut tokio::task::JoinSet<Result<(usize, BedrockLatencySession), String>>,
    addr: std::net::SocketAddr,
    index: usize,
) {
    connecting.spawn(async move {
        let view_group = index % LATENCY_VIEW_GROUP_COUNT;
        let x = (view_group + 1) as f32 * 96.0 + 0.5;
        tokio::time::timeout(
            BEDROCK_SESSION_CONNECT_TIMEOUT,
            BedrockLatencySession::connect(addr, &format!("b{index:04}"), x),
        )
        .await
        .map_err(|_| {
            format!(
                "Bedrock latency session {index} did not reach Play within {}s",
                BEDROCK_SESSION_CONNECT_TIMEOUT.as_secs()
            )
        })?
        .map(|session| (index, session))
        .map_err(|error| format!("failed to establish Bedrock latency session {index}: {error}"))
    });
}

async fn run_java_steady_workload(
    sessions: &[JavaLatencySession],
    iteration: usize,
) -> TestResult<()> {
    let slot = i16::try_from(iteration % 8)?;
    run_java_roundtrip(sessions, slot, Duration::from_secs(10)).await
}

async fn run_continuous_workload(
    java: &[JavaLatencySession],
    bedrock: &[BedrockLatencySession],
    mut sequence: usize,
    stop: watch::Receiver<bool>,
) -> TestResult<()> {
    let mut period = tokio::time::interval(LATENCY_WORKLOAD_PERIOD);
    period.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while !*stop.borrow() {
        period.tick().await;
        tokio::try_join!(
            run_java_steady_workload(java, sequence),
            run_bedrock_steady_workload(bedrock, sequence),
        )?;
        sequence = sequence.wrapping_add(1);
    }
    Ok(())
}

async fn run_bedrock_steady_workload(
    sessions: &[BedrockLatencySession],
    iteration: usize,
) -> TestResult<()> {
    let slot = i8::try_from(iteration % 8)?;
    run_bedrock_roundtrip(
        sessions,
        slot,
        Duration::from_secs(120),
        BedrockWorkloadKind::CompleteGameplay,
    )
    .await
}

async fn run_java_roundtrip(
    sessions: &[JavaLatencySession],
    slot: i16,
    timeout: Duration,
) -> TestResult<()> {
    // One outstanding request per session bounds the live workload without serializing whole
    // connection groups. All sessions must be active during the same cutover, not in turns.
    let mut responses = Vec::with_capacity(sessions.len());
    for (index, session) in sessions.iter().enumerate() {
        responses.push((index, session.begin_roundtrip(slot).await?));
    }
    for (index, response) in responses {
        tokio::time::timeout(timeout, response)
            .await
            .map_err(|_| format!("Java steady workload response timed out for session {index}"))?
            .map_err(|_| format!("Java steady workload actor {index} stopped"))?
            .map_err(|error| format!("Java steady workload session {index} failed: {error}"))?;
    }
    Ok(())
}

async fn run_bedrock_roundtrip(
    sessions: &[BedrockLatencySession],
    slot: i8,
    timeout: Duration,
    kind: BedrockWorkloadKind,
) -> TestResult<()> {
    let fan_out = match kind {
        BedrockWorkloadKind::Stabilize => BEDROCK_STABILIZATION_FAN_OUT,
        BedrockWorkloadKind::CompleteGameplay => LATENCY_SESSION_COUNT,
    };
    for (group_index, group) in sessions.chunks(fan_out).enumerate() {
        let first_index = group_index * fan_out;
        let mut responses = Vec::with_capacity(group.len());
        for (offset, session) in group.iter().enumerate() {
            responses.push((
                first_index + offset,
                session.begin_workload(kind, slot).await?,
            ));
        }
        for (index, response) in responses {
            tokio::time::timeout(timeout, response)
                .await
                .map_err(|_| {
                    format!(
                        "Bedrock {} response timed out for session {index}",
                        kind.name()
                    )
                })?
                .map_err(|_| format!("Bedrock steady workload actor {index} stopped"))?
                .map_err(|error| {
                    format!("Bedrock {} session {index} failed: {error}", kind.name())
                })?;
        }
    }
    Ok(())
}

async fn release_bedrock_cutover(sessions: &[BedrockLatencySession]) -> TestResult<()> {
    let mut responses = Vec::with_capacity(sessions.len());
    for (index, session) in sessions.iter().enumerate() {
        responses.push((index, session.begin_release_cutover().await?));
    }
    for (index, response) in responses {
        tokio::time::timeout(Duration::from_secs(10), response)
            .await
            .map_err(|_| format!("Bedrock cutover release timed out for session {index}"))?
            .map_err(|_| format!("Bedrock cutover release actor {index} stopped"))?
            .map_err(|error| format!("Bedrock cutover release session {index} failed: {error}"))?;
    }
    Ok(())
}

async fn arm_bedrock_cutover(sessions: &[BedrockLatencySession]) -> TestResult<()> {
    let mut responses = Vec::with_capacity(sessions.len());
    for (index, session) in sessions.iter().enumerate() {
        responses.push((index, session.begin_arm_in_flight().await?));
    }
    for (index, response) in responses {
        tokio::time::timeout(Duration::from_secs(10), response)
            .await
            .map_err(|_| format!("Bedrock in-flight marker timed out for session {index}"))?
            .map_err(|_| format!("Bedrock in-flight marker actor {index} stopped"))?
            .map_err(|error| format!("Bedrock in-flight marker session {index} failed: {error}"))?;
    }
    Ok(())
}

impl BedrockLatencySession {
    async fn connect(
        addr: std::net::SocketAddr,
        username: &str,
        isolated_x: f32,
    ) -> Result<Self, String> {
        let mut oracle = BedrockOracle::connect(addr)
            .await
            .map_err(|error| error.to_string())?;
        oracle
            .login(username)
            .await
            .map_err(|error| error.to_string())?;
        oracle
            .relocate(isolated_x)
            .await
            .map_err(|error| error.to_string())?;

        let (command_tx, command_rx) = mpsc::channel(1);
        let mut tasks = JoinSet::new();
        tasks.spawn(run_bedrock_latency_actor(
            username.to_string(),
            oracle,
            command_rx,
        ));
        Ok(Self { command_tx, tasks })
    }

    async fn begin_workload(
        &self,
        kind: BedrockWorkloadKind,
        slot: i8,
    ) -> TestResult<oneshot::Receiver<Result<(), String>>> {
        let (response_tx, response_rx) = oneshot::channel();
        let workload = match kind {
            BedrockWorkloadKind::Stabilize => {
                BedrockLatencyWorkload::Stabilize { slot, response_tx }
            }
            BedrockWorkloadKind::CompleteGameplay => {
                BedrockLatencyWorkload::CompleteGameplay { slot, response_tx }
            }
        };
        self.command_tx
            .send(workload)
            .await
            .map_err(|_| "Bedrock latency actor stopped before workload command")?;
        Ok(response_rx)
    }

    async fn begin_release_cutover(&self) -> TestResult<oneshot::Receiver<Result<(), String>>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(BedrockLatencyWorkload::ReleaseCutover { response_tx })
            .await
            .map_err(|_| "Bedrock latency actor stopped before cutover release")?;
        Ok(response_rx)
    }

    async fn begin_arm_in_flight(&self) -> TestResult<oneshot::Receiver<Result<(), String>>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(BedrockLatencyWorkload::ArmInFlight { response_tx })
            .await
            .map_err(|_| "Bedrock latency actor stopped before in-flight marker")?;
        Ok(response_rx)
    }
}

#[derive(Clone, Copy)]
enum BedrockWorkloadKind {
    Stabilize,
    CompleteGameplay,
}

impl BedrockWorkloadKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Stabilize => "stabilization",
            Self::CompleteGameplay => "steady workload",
        }
    }
}

async fn run_bedrock_latency_actor(
    session_name: String,
    mut oracle: BedrockOracle,
    mut command_rx: mpsc::Receiver<BedrockLatencyWorkload>,
) {
    loop {
        match oracle.receive_command(command_rx.recv()).await {
            Ok(Some(command)) => {
                let (response_tx, result) = match command {
                    BedrockLatencyWorkload::Stabilize { slot, response_tx } => {
                        let result = converge_bedrock_slot(&mut oracle, slot).await;
                        (response_tx, result)
                    }
                    BedrockLatencyWorkload::CompleteGameplay { slot, response_tx } => {
                        let result = converge_bedrock_slot(&mut oracle, slot).await;
                        (response_tx, result)
                    }
                    BedrockLatencyWorkload::ArmInFlight { response_tx } => {
                        let result = async {
                            oracle.begin_in_flight_marker().await?;
                            oracle.await_in_flight_marker().await
                        }
                        .await;
                        (response_tx, result)
                    }
                    BedrockLatencyWorkload::ReleaseCutover { response_tx } => {
                        let result = oracle.release_in_flight().await;
                        (response_tx, result)
                    }
                };
                if let Err(error) = &result {
                    eprintln!("Bedrock latency session {session_name} workload failed: {error}");
                }
                if response_tx
                    .send(result.map_err(|error: std::io::Error| error.to_string()))
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                eprintln!("Bedrock latency session {session_name} stopped while draining: {error}");
                break;
            }
        }
    }
}

async fn converge_bedrock_slot(oracle: &mut BedrockOracle, slot: i8) -> std::io::Result<()> {
    let wanted = u32::try_from(slot).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "negative Bedrock hotbar slot",
        )
    })?;
    for _ in 0..LATENCY_COMMAND_ATTEMPT_LIMIT {
        oracle.begin_roundtrip(slot).await?;
        // Reliable transport owns packet loss recovery. An application retry is appropriate
        // only after the response window, never for every unsolicited resync notification.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        match oracle
            .await_selected_slot(wanted, deadline, LATENCY_COMMAND_RESPONSE_WINDOW)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "Bedrock held-item command did not converge to slot {wanted} after {} response windows",
            LATENCY_COMMAND_ATTEMPT_LIMIT
        ),
    ))
}

impl JavaLatencySession {
    async fn connect(
        addr: std::net::SocketAddr,
        username: &str,
        isolated_x: f64,
    ) -> Result<Self, String> {
        let codec = MinecraftWireCodec;
        let protocol = TestJavaProtocol::Je5;
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|error| error.to_string())?;
        stream
            .set_nodelay(true)
            .map_err(|error| error.to_string())?;
        write_java_frame(
            &mut stream,
            &codec,
            &encode_handshake(protocol.protocol_version(), 2).map_err(|error| error.to_string())?,
        )
        .await?;
        write_java_frame(
            &mut stream,
            &codec,
            &login_start(username).map_err(|error| error.to_string())?,
        )
        .await?;

        let mut buffer = BytesMut::new();
        let mut received_window_items = false;
        let mut received_held_item = false;
        while !received_window_items || !received_held_item {
            let frame = tokio::time::timeout(
                Duration::from_secs(30),
                read_java_frame(&mut stream, &codec, &mut buffer),
            )
            .await
            .map_err(|_| "Java login bootstrap timed out".to_string())??;
            let id = packet_id(&frame).map_err(|error| error.to_string())?;
            received_window_items |=
                protocol.clientbound_packet_id(TestJavaPacket::WindowItems) == Some(id);
            received_held_item |=
                protocol.clientbound_packet_id(TestJavaPacket::HeldItemChange) == Some(id);
        }
        write_java_frame(
            &mut stream,
            &codec,
            &java_position_look(isolated_x, 4.0, 0.5, 0.0, 0.0),
        )
        .await?;

        let (command_tx, command_rx) = mpsc::channel(1);
        let (terminal_tx, terminal_error) = watch::channel(None);
        let username = username.to_string();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            if let Err(error) =
                run_java_latency_actor(stream, buffer, codec, protocol, command_rx).await
            {
                eprintln!("latency workload: Java session {username} stopped: {error}");
                terminal_tx.send_replace(Some(error));
            }
        });
        Ok(Self {
            command_tx,
            terminal_error,
            tasks,
        })
    }

    async fn begin_roundtrip(
        &self,
        slot: i16,
    ) -> TestResult<oneshot::Receiver<Result<(), String>>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(JavaLatencyCommand {
                slot,
                attempts: 0,
                response_tx,
            })
            .await
            .map_err(|_| {
                self.terminal_error.borrow().clone().map_or_else(
                    || "Java latency actor stopped before workload command".to_string(),
                    |error| format!("Java latency actor stopped before workload command: {error}"),
                )
            })?;
        Ok(response_rx)
    }
}

fn java_position_look(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_f64(x);
    writer.write_f64(y + 1.62);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

fn java_keep_alive_response(frame: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = PacketReader::new(frame);
    if reader.read_varint().map_err(|error| error.to_string())? != 0x00 {
        return Err("expected Java keepalive packet".to_string());
    }
    let keep_alive_id = reader.read_i32().map_err(|error| error.to_string())?;
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_i32(keep_alive_id);
    Ok(writer.into_inner())
}

async fn run_java_latency_actor(
    stream: tokio::net::TcpStream,
    mut buffer: BytesMut,
    codec: MinecraftWireCodec,
    protocol: TestJavaProtocol,
    mut command_rx: mpsc::Receiver<JavaLatencyCommand>,
) -> Result<(), String> {
    let (mut reader, mut writer) = stream.into_split();
    let mut pending: Option<JavaLatencyCommand> = None;
    let mut retry_deadline = None;
    loop {
        let wait_for_retry = async {
            match retry_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            command = command_rx.recv(), if pending.is_none() => {
                let Some(mut command) = command else { break Ok(()); };
                if let Err(error) = write_java_frame(
                    &mut writer,
                    &codec,
                    &held_item_change(command.slot),
                ).await {
                    let _ = command.response_tx.send(Err(error.clone()));
                    break Err(error);
                }
                command.attempts = 1;
                pending = Some(command);
            }
            () = wait_for_retry => {
                retry_deadline = None;
                let Some(mut command) = pending.take() else { continue; };
                if command.attempts >= LATENCY_COMMAND_ATTEMPT_LIMIT {
                    let _ = command.response_tx.send(Err(format!(
                        "held-item command did not converge to slot {} after {} response windows",
                        command.slot, LATENCY_COMMAND_ATTEMPT_LIMIT,
                    )));
                    continue;
                }
                if let Err(error) = write_java_frame(
                    &mut writer, &codec, &held_item_change(command.slot),
                ).await {
                    let _ = command.response_tx.send(Err(error.clone()));
                    break Err(error);
                }
                command.attempts += 1;
                pending = Some(command);
            }
            frame = read_java_frame(&mut reader, &codec, &mut buffer) => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        if let Some(command) = pending.take() {
                            let _ = command.response_tx.send(Err(error.clone()));
                        }
                        break Err(error);
                    }
                };
                let Ok(id) = packet_id(&frame) else { continue; };
                if id == 0x00 {
                    let response = match java_keep_alive_response(&frame) {
                        Ok(response) => response,
                        Err(error) => {
                            if let Some(command) = pending.take() {
                                let _ = command.response_tx.send(Err(error.clone()));
                            }
                            break Err(error);
                        }
                    };
                    if let Err(error) = write_java_frame(&mut writer, &codec, &response).await {
                        if let Some(command) = pending.take() {
                            let _ = command.response_tx.send(Err(error.clone()));
                        }
                        break Err(error);
                    }
                    continue;
                }
                let Some(command) = pending.as_ref() else { continue; };
                if protocol.clientbound_packet_id(TestJavaPacket::HeldItemChange) != Some(id) {
                    continue;
                }
                let actual = match held_item_from_packet(protocol, &frame)
                    .map(i16::from)
                    .map_err(|error| error.to_string()) {
                        Ok(actual) => actual,
                        Err(error) => {
                            if let Some(command) = pending.take() {
                                let _ = command.response_tx.send(Err(error));
                            }
                            continue;
                        }
                    };
                if actual != command.slot {
                    // Resync notifications are not correlated command acknowledgements.
                    retry_deadline.get_or_insert_with(|| {
                        tokio::time::Instant::now() + LATENCY_COMMAND_RESPONSE_WINDOW
                    });
                    continue;
                }
                if let Some(command) = pending.take() {
                    retry_deadline = None;
                    let _ = command.response_tx.send(Ok(()));
                }
            }
        }
    }
}

async fn write_java_frame<W>(
    stream: &mut W,
    codec: &MinecraftWireCodec,
    payload: &[u8],
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let frame = codec
        .encode_frame(payload)
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&frame)
        .await
        .map_err(|error| error.to_string())
}

// Cancellation at read_buf retains every received byte in the actor-owned buffer;
// decoding consumes a complete frame only when this future returns it without suspending.
async fn read_java_frame<R>(
    stream: &mut R,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
) -> Result<Vec<u8>, String>
where
    R: AsyncRead + Unpin,
{
    loop {
        if let Some(frame) = codec
            .try_decode_frame(buffer)
            .map_err(|error| error.to_string())?
        {
            return Ok(frame);
        }
        let bytes_read = stream
            .read_buf(buffer)
            .await
            .map_err(|error| error.to_string())?;
        if bytes_read == 0 {
            return Err("Java connection closed".to_string());
        }
    }
}

fn latency_sample(report: proto::CutoverReport) -> TestResult<LatencySample> {
    let operation = match proto::CutoverOperation::try_from(report.operation)? {
        proto::CutoverOperation::Reload => "reload",
        proto::CutoverOperation::ExecutableUpgrade => "executable-upgrade",
        proto::CutoverOperation::Unspecified => {
            return Err("cutover report operation was unspecified".into());
        }
    };
    let mode = report
        .mode
        .map(proto::RuntimeReloadMode::try_from)
        .transpose()?
        .map(|mode| match mode {
            proto::RuntimeReloadMode::Full => Ok("full"),
            proto::RuntimeReloadMode::Artifacts => Ok("artifacts"),
            proto::RuntimeReloadMode::Topology => Ok("topology"),
            proto::RuntimeReloadMode::Core => Ok("core"),
            proto::RuntimeReloadMode::Unspecified => {
                Err("latency workload received an unspecified reload mode")
            }
        })
        .transpose()?;
    let outcome = match proto::CutoverOutcome::try_from(report.outcome)? {
        proto::CutoverOutcome::Committed => "committed",
        proto::CutoverOutcome::Aborted => "aborted",
        proto::CutoverOutcome::Unspecified => "unspecified",
    };
    let mix = report
        .connection_mix
        .ok_or("cutover report omitted connection mix")?;
    Ok(LatencySample {
        operation,
        mode,
        session_count: report.session_count,
        connection_mix: LatencyConnectionMix {
            java: mix.java,
            bedrock: mix.bedrock,
        },
        stage_us: report.stage_us,
        prepare_us: report.prepare_us,
        freeze_us: report.freeze_us,
        resume_us: report.resume_us,
        outcome,
        epoch_revision: report.epoch_revision,
    })
}

#[tokio::test]
async fn java_frame_read_retains_partial_data_when_control_is_ready() -> TestResult<()> {
    let codec = MinecraftWireCodec;
    let payload = vec![0x35; 300];
    let encoded = codec.encode_frame(&payload)?;
    let (mut sender, mut receiver) = tokio::io::duplex(1024);
    let mut buffer = BytesMut::new();
    // Stop once inside the multi-byte length prefix and once inside its payload.
    for bytes in [&encoded[..1], &encoded[1..30]] {
        sender.write_all(bytes).await?;
        tokio::select! {
            biased;
            result = read_java_frame(&mut receiver, &codec, &mut buffer) => {
                return Err(format!("incomplete frame unexpectedly completed: {result:?}").into());
            }
            () = std::future::ready(()) => {}
        }
    }
    assert_eq!(&buffer[..], &encoded[..30]);
    sender.write_all(&encoded[30..]).await?;
    assert_eq!(
        read_java_frame(&mut receiver, &codec, &mut buffer).await?,
        payload
    );
    assert!(buffer.is_empty());
    Ok(())
}

#[tokio::test]
async fn java_hotbar_response_drains_stale_resync_without_retransmitting_commands() -> TestResult<()>
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = tokio::net::TcpStream::connect(listener.local_addr()?).await?;
    let (mut server, _) = listener.accept().await?;
    let (command_tx, command_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(JavaLatencyCommand {
            slot: 1,
            attempts: 0,
            response_tx,
        })
        .await?;
    let actor = run_java_latency_actor(
        client,
        BytesMut::new(),
        MinecraftWireCodec,
        TestJavaProtocol::Je5,
        command_rx,
    );
    let exchange = async {
        let mut buffer = BytesMut::new();
        let command = read_java_frame(&mut server, &MinecraftWireCodec, &mut buffer).await?;
        assert_eq!(command, held_item_change(1));
        assert!(
            tokio::time::timeout(
                LATENCY_COMMAND_RESPONSE_WINDOW + Duration::from_millis(50),
                read_java_frame(&mut server, &MinecraftWireCodec, &mut buffer),
            )
            .await
            .is_err(),
            "network silence must not arm the resync retry window"
        );
        for _ in 0..32 {
            write_java_frame(&mut server, &MinecraftWireCodec, &[0x09, 6]).await?;
        }
        write_java_frame(&mut server, &MinecraftWireCodec, &[0x09, 1]).await?;
        response_rx.await.map_err(|error| error.to_string())??;
        drop(command_tx);
        let mut trailing = Vec::new();
        server
            .read_to_end(&mut trailing)
            .await
            .map_err(|error| error.to_string())?;
        assert!(
            trailing.is_empty(),
            "old hotbar notifications must not cause command retransmissions"
        );
        Ok::<_, String>(())
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::try_join!(actor, exchange)
    })
    .await??;
    Ok(())
}
