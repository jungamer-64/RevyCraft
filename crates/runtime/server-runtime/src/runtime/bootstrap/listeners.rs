use crate::ListenerBinding;
use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::runtime::{
    AcceptedGenerationSession, GenerationId, QueuedAcceptTracker, TopologyListenerWorker,
};
use crate::transport::{
    AcceptedTransportSession, BoundTransportListener, TransportSessionIo, bind_transport_listener,
    build_listener_plans,
};
use mc_plugin_host::registry::ProtocolRegistry;
use mc_proto_common::TransportKind;
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, watch};

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
    let tcp_listener = bind_transport_listener(tcp_plan, config).await?;
    let tcp_local_addr = match &tcp_listener {
        BoundTransportListener::Tcp { listener, .. } => listener.local_addr()?,
        BoundTransportListener::Bedrock { .. } => {
            return Err(RuntimeError::Config(
                "tcp listener plan resolved to a non-tcp listener".to_string(),
            ));
        }
    };

    let mut bound_listeners = vec![tcp_listener];
    if let Some(mut udp_plan) = udp_plan {
        if udp_plan.bind_addr.port() == 0 {
            udp_plan.bind_addr = SocketAddr::new(tcp_local_addr.ip(), tcp_local_addr.port());
        }
        bound_listeners.push(bind_transport_listener(udp_plan, config).await?);
    }

    let listener_bindings = bound_listeners
        .iter()
        .map(BoundTransportListener::listener_binding)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(BoundListeners {
        listener_bindings,
        bound_listeners,
    })
}

pub(super) fn spawn_listener_workers(
    bound_listeners: Vec<BoundTransportListener>,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
) -> Result<HashMap<TransportKind, TopologyListenerWorker>, RuntimeError> {
    let mut workers = HashMap::new();
    for listener in bound_listeners {
        let worker = spawn_listener_worker(
            listener,
            generation_id,
            accepted_tx.clone(),
            queued_accepts.clone(),
        )?;
        if workers.insert(worker.transport, worker).is_some() {
            return Err(RuntimeError::Config(
                "multiple listener workers for the same transport are not supported".to_string(),
            ));
        }
    }
    Ok(workers)
}

pub(in crate::runtime) fn spawn_listener_worker(
    listener: BoundTransportListener,
    generation_id: GenerationId,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    queued_accepts: QueuedAcceptTracker,
) -> Result<TopologyListenerWorker, RuntimeError> {
    let binding = listener.listener_binding()?;
    let transport = binding.transport;
    let (generation_tx, generation_rx) = watch::channel(generation_id);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let join_handle = match listener {
        BoundTransportListener::Tcp { listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let generation_id = *generation_rx.borrow();
                        queued_accepts.increment(generation_id);
                        let session = AcceptedGenerationSession {
                            generation_id,
                            session: AcceptedTransportSession {
                                transport: TransportKind::Tcp,
                                io: TransportSessionIo::Tcp {
                                    stream,
                                    encryption: Box::default(),
                                },
                            },
                        };
                        if let Err(error) = accepted_tx.try_send(session) {
                            queued_accepts.decrement(generation_id);
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
        BoundTransportListener::Bedrock { mut listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok(connection) = accepted else {
                            break;
                        };
                        let generation_id = *generation_rx.borrow();
                        queued_accepts.increment(generation_id);
                        let session = AcceptedGenerationSession {
                            generation_id,
                            session: AcceptedTransportSession {
                                transport: TransportKind::Udp,
                                io: TransportSessionIo::Bedrock {
                                    connection,
                                    compression: None,
                                },
                            },
                        };
                        if let Err(error) = accepted_tx.try_send(session) {
                            queued_accepts.decrement(generation_id);
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
        }),
    };
    Ok(TopologyListenerWorker {
        transport,
        generation_tx,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
    })
}
