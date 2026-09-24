use crate::ListenerBinding;
use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::runtime::{
    AcceptedGenerationSession, ExecutableListenerResource, GenerationId, ListenerIngressCommand,
    QueuedAcceptTracker, TopologyListenerWorker,
};
use crate::transport::{
    AcceptedTransportSession, BedrockListenerSocket, BoundTransportListener, TransportSessionIo,
    bind_transport_listener, build_listener_plans,
};
use mc_plugin_host::registry::ProtocolRegistry;
use mc_proto_common::TransportKind;
use rand_core::{OsRng, RngCore};
use revy_raknet::{
    Frozen as RakNetFrozen, RakNetServer, ReceivePaused as RakNetReceivePaused,
    Serving as RakNetServing,
};
use revy_runtime_transfer::{ExportedSocket, SocketTransferTarget};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, watch};

const EPHEMERAL_DUAL_TRANSPORT_BIND_ATTEMPTS: usize = 32;
const DYNAMIC_PRIVATE_PORT_START: u16 = 49_152;
const DYNAMIC_PRIVATE_PORT_COUNT: u16 = 16_384;
// One candidate per roughly 512-port band; the odd stride visits every port before repeating.
const DYNAMIC_PRIVATE_PORT_STRIDE: u16 = 513;

pub(super) struct BoundListeners {
    pub(super) listener_bindings: Vec<ListenerBinding>,
    pub(super) bound_listeners: Vec<BoundTransportListener>,
}

pub(super) async fn bind_runtime_listeners(
    config: &ServerConfig,
    active_protocols: &ProtocolRegistry,
) -> Result<BoundListeners, RuntimeError> {
    let listener_plans = build_listener_plans(config, active_protocols)?;
    let mut tcp_plan = None;
    let mut udp_plan = None;
    for plan in listener_plans {
        match plan.transport {
            TransportKind::Tcp => tcp_plan = Some(plan),
            TransportKind::Udp => udp_plan = Some(plan),
        }
    }
    let tcp_plan = tcp_plan
        .ok_or_else(|| RuntimeError::Config("no tcp listener plan was generated".to_string()))?;
    let bound_listeners = if udp_plan.is_some() && tcp_plan.bind_addr.port() == 0 {
        bind_ephemeral_transport_pair(tcp_plan, udp_plan.expect("udp plan was checked"), config)
            .await?
    } else {
        let tcp_listener = bind_transport_listener(tcp_plan, config).await?;
        let tcp_local_addr = tcp_listener_addr(&tcp_listener)?;
        let mut listeners = vec![tcp_listener];
        if let Some(mut udp_plan) = udp_plan {
            if udp_plan.bind_addr.port() == 0 {
                udp_plan.bind_addr = SocketAddr::new(tcp_local_addr.ip(), tcp_local_addr.port());
            }
            listeners.push(bind_transport_listener(udp_plan, config).await?);
        }
        listeners
    };
    let listener_bindings = bound_listeners
        .iter()
        .map(BoundTransportListener::listener_binding)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BoundListeners {
        listener_bindings,
        bound_listeners,
    })
}

fn tcp_listener_addr(listener: &BoundTransportListener) -> Result<SocketAddr, RuntimeError> {
    match listener {
        BoundTransportListener::Tcp { listener, .. } => Ok(listener.local_addr()?),
        BoundTransportListener::Bedrock { .. } => Err(RuntimeError::Config(
            "tcp listener plan resolved to a non-tcp listener".to_string(),
        )),
    }
}

/// TCP and UDP have independent port reservations. Spread candidates across the dynamic/private
/// range so a contiguous reservation in either protocol cannot consume the attempt budget.
/// Both sockets must bind before this inactive listener pair is returned.
pub(in crate::runtime) async fn bind_ephemeral_transport_pair(
    tcp_plan: crate::transport::ListenerPlan,
    udp_plan: crate::transport::ListenerPlan,
    config: &ServerConfig,
) -> Result<Vec<BoundTransportListener>, RuntimeError> {
    let mut random_bytes = [0; 2];
    OsRng
        .try_fill_bytes(&mut random_bytes)
        .map_err(|error| RuntimeError::Io(std::io::Error::other(error)))?;
    let mut offset = u16::from_ne_bytes(random_bytes) % DYNAMIC_PRIVATE_PORT_COUNT;
    for _ in 0..EPHEMERAL_DUAL_TRANSPORT_BIND_ATTEMPTS {
        let port = DYNAMIC_PRIVATE_PORT_START + offset;
        offset = (offset + DYNAMIC_PRIVATE_PORT_STRIDE) % DYNAMIC_PRIVATE_PORT_COUNT;
        let mut tcp_candidate = tcp_plan.clone();
        tcp_candidate.bind_addr.set_port(port);
        let tcp = match bind_transport_listener(tcp_candidate, config).await {
            Ok(tcp) => tcp,
            Err(error) if transient_ephemeral_pair_collision(&error) => continue,
            Err(error) => return Err(error),
        };
        let mut udp_candidate = udp_plan.clone();
        udp_candidate.bind_addr.set_port(port);
        match bind_transport_listener(udp_candidate, config).await {
            Ok(udp) => return Ok(vec![tcp, udp]),
            Err(error) if transient_ephemeral_pair_collision(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(RuntimeError::BudgetExceeded {
        resource: "ephemeral TCP/UDP listener pair bind attempts",
        requested: EPHEMERAL_DUAL_TRANSPORT_BIND_ATTEMPTS + 1,
        limit: EPHEMERAL_DUAL_TRANSPORT_BIND_ATTEMPTS,
    })
}

fn transient_ephemeral_pair_collision(error: &RuntimeError) -> bool {
    match error {
        RuntimeError::Io(error) => {
            error.kind() == std::io::ErrorKind::AddrInUse
                || cfg!(windows)
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                    && error.raw_os_error() == Some(10013)
        }
        RuntimeError::PluginLoad(_)
        | RuntimeError::PluginFatal(_)
        | RuntimeError::Protocol(_)
        | RuntimeError::Storage(_)
        | RuntimeError::CoreTransfer(_)
        | RuntimeError::CutoverEventEncoding(_)
        | RuntimeError::Allocation { .. }
        | RuntimeError::Auth(_)
        | RuntimeError::RakNet(_)
        | RuntimeError::SocketTransfer(_)
        | RuntimeError::Unsupported(_)
        | RuntimeError::Config(_)
        | RuntimeError::CutoverOutpaced { .. }
        | RuntimeError::CutoverDirectoryChanged { .. }
        | RuntimeError::CutoverAbortFailed { .. }
        | RuntimeError::BudgetExceeded { .. }
        | RuntimeError::Join(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{BedrockBindMetadata, ListenerPlan};

    #[tokio::test]
    async fn ephemeral_pair_owns_one_port_and_releases_both_sockets() -> Result<(), RuntimeError> {
        let bind_addr = "127.0.0.1:0".parse().expect("loopback socket address");
        let tcp = ListenerPlan {
            transport: TransportKind::Tcp,
            bind_addr,
            adapter_ids: Vec::new(),
            bedrock_bind_metadata: None,
        };
        let udp = ListenerPlan {
            transport: TransportKind::Udp,
            bind_addr,
            adapter_ids: Vec::new(),
            bedrock_bind_metadata: Some(BedrockBindMetadata {
                game_version: "1.26.0".to_string(),
                protocol_number: 924,
                raknet_version: 11,
            }),
        };
        let listeners = bind_ephemeral_transport_pair(tcp, udp, &ServerConfig::default()).await?;
        assert_eq!(listeners.len(), 2);
        let tcp_binding = listeners[0].listener_binding()?;
        let udp_binding = listeners[1].listener_binding()?;
        assert_eq!(tcp_binding.transport, TransportKind::Tcp);
        assert_eq!(udp_binding.transport, TransportKind::Udp);
        assert_eq!(tcp_binding.local_addr, udp_binding.local_addr);
        assert_ne!(tcp_binding.local_addr.port(), 0);
        drop(listeners);
        let _tcp = tokio::net::TcpListener::bind(tcp_binding.local_addr).await?;
        let _udp = tokio::net::UdpSocket::bind(udp_binding.local_addr).await?;
        Ok(())
    }

    #[test]
    fn pair_search_retries_socket_collisions_but_not_other_resource_failures() {
        for error in [
            RuntimeError::Io(std::io::ErrorKind::AddrInUse.into()),
            RuntimeError::from(revy_raknet::RakNetError::Io(
                std::io::ErrorKind::AddrInUse.into(),
            )),
        ] {
            assert!(transient_ephemeral_pair_collision(&error));
        }
        assert!(!transient_ephemeral_pair_collision(&RuntimeError::Io(
            std::io::ErrorKind::OutOfMemory.into(),
        )));
        assert!(!transient_ephemeral_pair_collision(&RuntimeError::from(
            revy_raknet::RakNetError::Closed,
        )));
    }
}

pub(super) fn spawn_listener_workers(
    bound_listeners: Vec<BoundTransportListener>,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
) -> Result<HashMap<TransportKind, TopologyListenerWorker>, RuntimeError> {
    spawn_listener_workers_with_state(
        bound_listeners,
        generation_id,
        accepted_tx,
        queued_accepts,
        true,
    )
}

pub(super) fn spawn_paused_listener_workers(
    bound_listeners: Vec<BoundTransportListener>,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
) -> Result<HashMap<TransportKind, TopologyListenerWorker>, RuntimeError> {
    spawn_listener_workers_with_state(
        bound_listeners,
        generation_id,
        accepted_tx,
        queued_accepts,
        false,
    )
}

fn spawn_listener_workers_with_state(
    bound_listeners: Vec<BoundTransportListener>,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
    serving: bool,
) -> Result<HashMap<TransportKind, TopologyListenerWorker>, RuntimeError> {
    let mut workers = HashMap::new();
    for listener in bound_listeners {
        let worker = spawn_listener_worker_with_state(
            listener,
            generation_id,
            accepted_tx.clone(),
            queued_accepts.clone(),
            serving,
        )?;
        if workers.insert(worker.transport, worker).is_some() {
            return Err(RuntimeError::Config(
                "multiple listener workers for the same transport are not supported".to_string(),
            ));
        }
    }
    Ok(workers)
}

pub(in crate::runtime) fn spawn_paused_listener_worker(
    listener: BoundTransportListener,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
) -> Result<TopologyListenerWorker, RuntimeError> {
    spawn_listener_worker_with_state(listener, generation_id, accepted_tx, queued_accepts, false)
}

fn spawn_listener_worker_with_state(
    listener: BoundTransportListener,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
    serving: bool,
) -> Result<TopologyListenerWorker, RuntimeError> {
    let binding = listener.listener_binding()?;
    let transport = binding.transport;
    let (generation_tx, generation_rx) = watch::channel(generation_id);
    let (serving_tx, serving_rx) = watch::channel(serving);
    let (ingress_tx, mut ingress_rx) = mpsc::channel(2);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let join_handle = match listener {
        BoundTransportListener::Tcp { listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            let mut serving_rx = serving_rx;
            let mut ingress_paused = false;
            loop {
                if !*serving_rx.borrow() || ingress_paused {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        changed = serving_rx.changed() => {
                            if changed.is_err() { break; }
                        }
                        command = ingress_rx.recv() => {
                            let Some(command) = command else { break; };
                            apply_tcp_ingress_command(
                                command,
                                &listener,
                                &binding,
                                &mut ingress_paused,
                            );
                        }
                    }
                    continue;
                }
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    changed = serving_rx.changed() => {
                        if changed.is_err() { break; }
                    }
                    command = ingress_rx.recv() => {
                        let Some(command) = command else { break; };
                        apply_tcp_ingress_command(
                            command,
                            &listener,
                            &binding,
                            &mut ingress_paused,
                        );
                    }
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let generation_id = *generation_rx.borrow();
                        let session = AcceptedGenerationSession::new(
                            generation_id,
                            AcceptedTransportSession {
                                transport: TransportKind::Tcp,
                                io: TransportSessionIo::tcp(stream),
                            },
                            queued_accepts.track(generation_id),
                        );
                        if let Err(error) = accepted_tx.try_send(session) {
                            match error {
                                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                    eprintln!("dropping tcp session because the accept queue is full");
                                }
                                tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
                            }
                        }
                    }
                }
            }
        }),
        BoundTransportListener::Bedrock { listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            let mut serving_rx = serving_rx;
            let mut ingress_paused = false;
            let mut authority = match (listener, serving) {
                (BedrockListenerSocket::Bound(listener), true) => {
                    BedrockListenerAuthority::Serving(Box::new((*listener).start()))
                }
                (BedrockListenerSocket::Bound(listener), false) => {
                    BedrockListenerAuthority::Paused(Box::new((*listener).start_paused()))
                }
                (BedrockListenerSocket::ReceivePaused(listener), false) => {
                    BedrockListenerAuthority::Paused(listener)
                }
                (BedrockListenerSocket::ReceivePaused(_), true) => {
                    unreachable!("imported Bedrock listeners must begin without receive authority")
                }
            };
            loop {
                let receive_enabled = *serving_rx.borrow() && !ingress_paused;
                if let Err(error) = set_bedrock_receive(&mut authority, receive_enabled).await {
                    eprintln!("bedrock listener receive transition failed: {error}");
                    break;
                }
                match &mut authority {
                    BedrockListenerAuthority::Paused(_) => {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            changed = serving_rx.changed() => {
                                if changed.is_err() { break; }
                            }
                            command = ingress_rx.recv() => {
                                let Some(command) = command else { break; };
                                let published = *serving_rx.borrow();
                                apply_bedrock_ingress_command(
                                    command,
                                    &mut authority,
                                    &mut ingress_paused,
                                    published,
                                    &binding,
                                ).await;
                            }
                        }
                    }
                    BedrockListenerAuthority::Serving(listener) => {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            changed = serving_rx.changed() => {
                                if changed.is_err() { break; }
                            }
                            command = ingress_rx.recv() => {
                                let Some(command) = command else { break; };
                                let published = *serving_rx.borrow();
                                apply_bedrock_ingress_command(
                                    command,
                                    &mut authority,
                                    &mut ingress_paused,
                                    published,
                                    &binding,
                                ).await;
                            }
                            accepted = listener.accept() => {
                                let Ok(connection) = accepted else { break; };
                                let generation_id = *generation_rx.borrow();
                                let session = AcceptedGenerationSession::new(
                                    generation_id,
                                    AcceptedTransportSession {
                                        transport: TransportKind::Udp,
                                        io: TransportSessionIo::bedrock(connection),
                                    },
                                    queued_accepts.track(generation_id),
                                );
                                if let Err(error) = accepted_tx.try_send(session) {
                                    match error {
                                        tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                            eprintln!("dropping bedrock session because the accept queue is full");
                                        }
                                        tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
                                    }
                                }
                            }
                        }
                    }
                    BedrockListenerAuthority::Frozen(_) => {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            command = ingress_rx.recv() => {
                                let Some(command) = command else { break; };
                                let published = *serving_rx.borrow();
                                apply_bedrock_ingress_command(
                                    command,
                                    &mut authority,
                                    &mut ingress_paused,
                                    published,
                                    &binding,
                                ).await;
                            }
                        }
                    }
                    BedrockListenerAuthority::Transitioning => {
                        unreachable!("bedrock listener transition must finish before polling")
                    }
                }
            }
        }),
    };
    Ok(TopologyListenerWorker {
        transport,
        generation_tx,
        serving_tx,
        ingress_tx,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
    })
}

fn apply_tcp_ingress_command(
    command: ListenerIngressCommand,
    listener: &tokio::net::TcpListener,
    binding: &ListenerBinding,
    ingress_paused: &mut bool,
) {
    match command {
        ListenerIngressCommand::Pause { ack_tx } => {
            *ingress_paused = true;
            let _ = ack_tx.send(Ok(()));
        }
        ListenerIngressCommand::Resume { ack_tx } => {
            *ingress_paused = false;
            let _ = ack_tx.send(Ok(()));
        }
        ListenerIngressCommand::DuplicateForProcessTransfer { target, ack_tx } => {
            let result = duplicate_socket(listener, target)
                .map(|socket| ExecutableListenerResource::new(binding.clone(), socket));
            let _ = ack_tx.send(result);
        }
        ListenerIngressCommand::SealProcessTransfer { ack_tx } => {
            let _ = ack_tx.send(Ok(Vec::new()));
        }
    }
}

enum BedrockListenerAuthority {
    Serving(Box<RakNetServer<RakNetServing>>),
    Paused(Box<RakNetServer<RakNetReceivePaused>>),
    Frozen(Box<RakNetServer<RakNetFrozen>>),
    Transitioning,
}

async fn set_bedrock_receive(
    authority: &mut BedrockListenerAuthority,
    enabled: bool,
) -> Result<(), RuntimeError> {
    let current = std::mem::replace(authority, BedrockListenerAuthority::Transitioning);
    *authority = match (enabled, current) {
        (true, BedrockListenerAuthority::Paused(listener)) => {
            BedrockListenerAuthority::Serving(Box::new(listener.resume_receive().await?))
        }
        (false, BedrockListenerAuthority::Serving(listener)) => {
            BedrockListenerAuthority::Paused(Box::new(listener.pause_receive().await?))
        }
        (true, BedrockListenerAuthority::Frozen(listener)) => {
            BedrockListenerAuthority::Serving(Box::new(listener.resume().await?))
        }
        (false, frozen @ BedrockListenerAuthority::Frozen(_)) => frozen,
        (true, serving @ BedrockListenerAuthority::Serving(_)) => serving,
        (false, paused @ BedrockListenerAuthority::Paused(_)) => paused,
        (_, BedrockListenerAuthority::Transitioning) => {
            return Err(RuntimeError::Config(
                "bedrock listener entered an overlapping receive transition".to_string(),
            ));
        }
    };
    Ok(())
}

async fn apply_bedrock_ingress_command(
    command: ListenerIngressCommand,
    authority: &mut BedrockListenerAuthority,
    ingress_paused: &mut bool,
    published: bool,
    binding: &ListenerBinding,
) {
    let (pause, ack_tx) = match command {
        ListenerIngressCommand::Pause { ack_tx } => (true, ack_tx),
        ListenerIngressCommand::Resume { ack_tx } => (false, ack_tx),
        ListenerIngressCommand::DuplicateForProcessTransfer { target, ack_tx } => {
            let result = duplicate_bedrock_socket(authority, target)
                .map(|socket| ExecutableListenerResource::new(binding.clone(), socket));
            let _ = ack_tx.send(result);
            return;
        }
        ListenerIngressCommand::SealProcessTransfer { ack_tx } => {
            let current = std::mem::replace(authority, BedrockListenerAuthority::Transitioning);
            match current {
                BedrockListenerAuthority::Paused(listener) => match listener.freeze().await {
                    Ok(listener) => {
                        let snapshot = listener.snapshot().peers.clone();
                        *authority = BedrockListenerAuthority::Frozen(Box::new(listener));
                        let _ = ack_tx.send(Ok(snapshot));
                    }
                    Err(failure) => {
                        let (listener, error) = failure.into_parts();
                        *authority = BedrockListenerAuthority::Paused(Box::new(listener));
                        let _ = ack_tx.send(Err(RuntimeError::from(error)));
                    }
                },
                frozen @ BedrockListenerAuthority::Frozen(_) => {
                    *authority = frozen;
                    let _ = ack_tx.send(Err(RuntimeError::Config(
                        "bedrock listener process-transfer state was already sealed".to_string(),
                    )));
                }
                other => {
                    *authority = other;
                    let _ = ack_tx.send(Err(RuntimeError::Config(
                        "bedrock listener must withhold receive authority before process-transfer seal"
                            .to_string(),
                    )));
                }
            }
            return;
        }
    };
    *ingress_paused = pause;
    let result = set_bedrock_receive(authority, published && !pause).await;
    let _ = ack_tx.send(result);
}

#[cfg(unix)]
fn duplicate_socket(
    socket: &impl std::os::fd::AsFd,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    ExportedSocket::duplicate(socket, target).map_err(RuntimeError::from)
}

#[cfg(windows)]
fn duplicate_socket(
    socket: &impl std::os::windows::io::AsRawSocket,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    ExportedSocket::duplicate(socket, target).map_err(RuntimeError::from)
}

fn duplicate_bedrock_socket(
    authority: &BedrockListenerAuthority,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    match authority {
        BedrockListenerAuthority::Serving(listener) => duplicate_socket(listener.as_ref(), target),
        BedrockListenerAuthority::Paused(listener) => duplicate_socket(listener.as_ref(), target),
        BedrockListenerAuthority::Frozen(listener) => duplicate_socket(listener.as_ref(), target),
        BedrockListenerAuthority::Transitioning => Err(RuntimeError::Config(
            "bedrock listener cannot be duplicated during a receive transition".to_string(),
        )),
    }
}
