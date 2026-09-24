use crate::RuntimeError;
use crate::runtime::executable::{
    ExecutableQueuedSessionMessage, FrozenProcessSessionState, PreparedImportedSession,
    capture_executable_session, executable_session_slot_capacity, executable_session_state,
};
use crate::runtime::session_registry::SessionRegistration;
use crate::runtime::{
    AcceptedGenerationSession, BoundSession, GenerationAdmission, LoginSession,
    PendingSessionState, PlaySession, QueuedAcceptGuard, RuntimeServer,
    SESSION_OUTBOUND_QUEUE_CAPACITY, SessionControl, SessionDirectoryProjection, SessionHandle,
    SessionLifecycle, SessionMessage, SessionPhase,
};
use crate::transport::{
    AcceptedTransportSession, FrozenProcessTransport, TransportSessionIo, default_wire_codec,
};
use bytes::BytesMut;
use mc_proto_common::{TransportKind, WireCodec};
use revy_runtime_transfer::ArenaReservation;
use revy_voxel_core::ConnectionId;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, oneshot};

impl RuntimeServer {
    pub(in crate::runtime) async fn spawn_imported_sessions(
        self: &Arc<Self>,
        imported: Vec<PreparedImportedSession>,
        child_epoch_revision: u64,
    ) -> Result<Vec<oneshot::Receiver<Result<(), String>>>, RuntimeError> {
        let mut registrations = Vec::with_capacity(imported.len());
        let mut activation_receivers = Vec::with_capacity(imported.len());
        for imported in imported {
            let PreparedImportedSession { actor, phase, io } = imported;
            let connection_id = actor.connection_id;
            let (tx, rx) = mpsc::channel(SESSION_OUTBOUND_QUEUE_CAPACITY);
            for message in actor.queued_messages {
                let message = match message {
                    ExecutableQueuedSessionMessage::Events(events) => {
                        SessionMessage::Events(events.into_iter().map(Arc::new).collect())
                    }
                    ExecutableQueuedSessionMessage::Terminate { reason } => {
                        SessionMessage::Terminate { reason }
                    }
                };
                tx.try_send(message).map_err(|error| {
                    RuntimeError::Config(format!(
                        "imported session {connection_id:?} exceeded its validated message queue: {error}"
                    ))
                })?;
            }
            let (control_tx, control_rx) = mpsc::channel(8);
            if let Some(reason) = actor.pending_terminate {
                control_tx
                    .try_send(SessionControl::Terminate { reason })
                    .map_err(|error| {
                        RuntimeError::Config(format!(
                            "imported session {connection_id:?} could not restore termination state: {error}"
                        ))
                    })?;
            }
            let directory = self
                .sessions
                .directory_projection(connection_id, phase.directory_entry());
            self.sessions.observe_connection_id(connection_id);
            let lifecycle = SessionLifecycle::TransferFrozen {
                active_epoch_revision: actor.epoch_revision,
                active: phase.clone(),
                pending: PendingSessionState {
                    epoch_revision: child_epoch_revision,
                    phase,
                    resync_frames: Vec::new(),
                },
            };
            let epoch_latch = self.authority.subscribe_epoch();
            let (activation_tx, activation_rx) = oneshot::channel();
            activation_receivers.push(activation_rx);
            let server = Arc::clone(self);
            let task_directory = Arc::clone(&directory);
            let task = async move {
                (
                    connection_id,
                    server
                        .run_session(
                            connection_id,
                            io,
                            lifecycle,
                            task_directory,
                            BytesMut::from(actor.read_buffer.as_slice()),
                            rx,
                            control_rx,
                            epoch_latch,
                            Some(activation_tx),
                        )
                        .await,
                )
            };
            registrations.push(SessionRegistration::new(
                connection_id,
                SessionHandle {
                    tx,
                    control_tx,
                    directory,
                },
                task,
            ));
        }
        self.sessions.register_imported(registrations).await?;
        Ok(activation_receivers)
    }

    pub(in crate::runtime) async fn spawn_accepted_transport_session(
        self: &Arc<Self>,
        accepted: AcceptedGenerationSession,
    ) {
        let _data_plane = self.authority.enter_data_plane().await;
        self.spawn_transport_session_inner(
            accepted.generation_id,
            accepted.session,
            Some(accepted.queued_accept),
        )
        .await;
    }

    async fn spawn_transport_session_inner(
        self: &Arc<Self>,
        generation_id: crate::runtime::GenerationId,
        transport_session: AcceptedTransportSession,
        queued_accept_guard: Option<QueuedAcceptGuard>,
    ) {
        if self.reload.is_shutting_down() {
            return;
        }
        let generation = match self.generation_admission(generation_id) {
            GenerationAdmission::Active(generation) | GenerationAdmission::Draining(generation) => {
                generation
            }
            GenerationAdmission::ExpiredDraining => {
                eprintln!(
                    "dropping transport session because generation {generation_id:?} finished draining before admission"
                );
                return;
            }
            GenerationAdmission::Missing => {
                eprintln!(
                    "dropping transport session because generation {generation_id:?} is retired"
                );
                return;
            }
        };
        let phase = match transport_session.transport {
            TransportKind::Tcp => SessionPhase::Handshaking {
                generation: Arc::clone(&generation),
            },
            TransportKind::Udp => {
                let Some(adapter) = generation.default_bedrock_adapter.clone() else {
                    eprintln!("dropping bedrock session because no default adapter is active");
                    return;
                };
                let gameplay = match self
                    .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
                    .await
                {
                    Ok(gameplay) => gameplay,
                    Err(error) => {
                        eprintln!(
                            "dropping bedrock session because gameplay profile could not resolve: {error}"
                        );
                        return;
                    }
                };
                SessionPhase::Login(LoginSession::Negotiating(BoundSession::new(
                    Arc::clone(&generation),
                    TransportKind::Udp,
                    adapter,
                    gameplay,
                    None,
                )))
            }
        };
        self.spawn_session(transport_session, phase).await;
        drop(queued_accept_guard);
    }

    async fn spawn_session(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        phase: SessionPhase,
    ) {
        let connection_id = self.sessions.next_connection_id().await;
        let (tx, rx) = mpsc::channel(SESSION_OUTBOUND_QUEUE_CAPACITY);
        let (control_tx, control_rx) = mpsc::channel(8);
        let directory = self
            .sessions
            .directory_projection(connection_id, phase.directory_entry());
        self.sessions.observe_connection_id(connection_id);
        self.sessions
            .insert(connection_id, tx, control_tx, Arc::clone(&directory))
            .await;

        let server = Arc::clone(self);
        let epoch_revision = server.authority.active().revision();
        let epoch_latch = server.authority.subscribe_epoch();
        self.sessions
            .spawn_task(async move {
                (
                    connection_id,
                    server
                        .run_session(
                            connection_id,
                            transport_session.io,
                            SessionLifecycle::Running {
                                epoch_revision,
                                phase,
                            },
                            directory,
                            BytesMut::with_capacity(8192),
                            rx,
                            control_rx,
                            epoch_latch,
                            None,
                        )
                        .await,
                )
            })
            .await;
    }

    fn running_phase(lifecycle: &SessionLifecycle) -> Result<&SessionPhase, RuntimeError> {
        match lifecycle {
            SessionLifecycle::Running { phase, .. }
            | SessionLifecycle::CutoverPrepared { active: phase, .. } => Ok(phase),
            SessionLifecycle::TransferFrozen { .. } | SessionLifecycle::Transferred => {
                Err(RuntimeError::Config(
                    "data-plane access attempted outside the running session lifecycle".to_string(),
                ))
            }
        }
    }

    fn running_phase_mut(
        lifecycle: &mut SessionLifecycle,
    ) -> Result<&mut SessionPhase, RuntimeError> {
        match lifecycle {
            SessionLifecycle::Running { phase, .. }
            | SessionLifecycle::CutoverPrepared { active: phase, .. } => Ok(phase),
            SessionLifecycle::TransferFrozen { .. } | SessionLifecycle::Transferred => {
                Err(RuntimeError::Config(
                    "data-plane mutation attempted outside the running session lifecycle"
                        .to_string(),
                ))
            }
        }
    }

    fn publish_directory(phase: &SessionPhase, directory: &SessionDirectoryProjection) {
        directory.publish(phase.directory_entry());
    }

    async fn process_read_buffer(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        lifecycle: &mut SessionLifecycle,
        directory: &SessionDirectoryProjection,
        read_buffer: &mut BytesMut,
    ) -> Result<bool, RuntimeError> {
        loop {
            let phase = Self::running_phase(lifecycle)?;
            let adapter = phase.binding().map(|binding| Arc::clone(&binding.adapter));
            let codec: &dyn WireCodec = match adapter.as_ref() {
                Some(current) => current.wire_codec(),
                None => default_wire_codec(phase.transport())?,
            };
            let Some(frame) = codec.try_decode_frame(read_buffer)? else {
                break;
            };
            let phase = Self::running_phase_mut(lifecycle)?;
            let should_close = self
                .handle_incoming_frame(connection_id, transport_io, phase, frame)
                .await?;
            Self::publish_directory(phase, directory);
            if should_close {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        mut transport_io: TransportSessionIo,
        mut lifecycle: SessionLifecycle,
        directory: Arc<SessionDirectoryProjection>,
        mut read_buffer: BytesMut,
        mut rx: mpsc::Receiver<SessionMessage>,
        mut control_rx: mpsc::Receiver<SessionControl>,
        mut epoch_latch: tokio::sync::watch::Receiver<u64>,
        mut imported_activation: Option<oneshot::Sender<Result<(), String>>>,
    ) -> Result<(), RuntimeError> {
        let mut writer_failure = transport_io.subscribe_writer_failure();
        let mut process_transfer = None;
        let mut process_transfer_slot: Option<ArenaReservation> = None;
        let result = async {
            let mut pending_messages = VecDeque::new();
            let mut pending_terminate = None;
            'session: loop {
                loop {
                    match control_rx.try_recv() {
                        Ok(control @ SessionControl::Terminate { .. }) => {
                            pending_terminate = Some(control);
                        }
                        Ok(control @ SessionControl::EnforceGenerationDrain) => {
                            pending_terminate.get_or_insert(control);
                        }
                        Ok(control) => {
                            if self
                                .handle_session_control(
                                    connection_id,
                                    &mut transport_io,
                                    &mut lifecycle,
                                    &directory,
                                    &mut process_transfer,
                                    &mut process_transfer_slot,
                                    &mut writer_failure,
                                    &read_buffer,
                                    &mut rx,
                                    &mut pending_messages,
                                    &pending_terminate,
                                    control,
                                )
                                .await?
                            {
                                return Ok(());
                            }
                        }
                        Err(TryRecvError::Disconnected) => break 'session,
                        Err(TryRecvError::Empty) => break,
                    }
                }

                if process_transfer.is_some() {
                    let Some(control) = control_rx.recv().await else {
                        break 'session;
                    };
                    match control {
                        control @ SessionControl::Terminate { .. } => {
                            pending_terminate = Some(control);
                        }
                        control @ SessionControl::EnforceGenerationDrain => {
                            pending_terminate.get_or_insert(control);
                        }
                        control => {
                            if self
                                .handle_session_control(
                                    connection_id,
                                    &mut transport_io,
                                    &mut lifecycle,
                                    &directory,
                                    &mut process_transfer,
                                    &mut process_transfer_slot,
                                    &mut writer_failure,
                                    &read_buffer,
                                    &mut rx,
                                    &mut pending_messages,
                                    &pending_terminate,
                                    control,
                                )
                                .await?
                            {
                                return Ok(());
                            }
                        }
                    }
                    continue;
                }

                if matches!(lifecycle, SessionLifecycle::TransferFrozen { .. }) {
                    tokio::select! {
                        biased;
                        control = control_rx.recv() => {
                            let Some(control) = control else {
                                break 'session;
                            };
                            match control {
                                control @ SessionControl::Terminate { .. } => {
                                    pending_terminate = Some(control);
                                }
                                control @ SessionControl::EnforceGenerationDrain => {
                                    pending_terminate.get_or_insert(control);
                                }
                                control => {
                                    if self.handle_session_control(
                                        connection_id,
                                        &mut transport_io,
                                        &mut lifecycle,
                                        &directory,
                                        &mut process_transfer,
                                        &mut process_transfer_slot,
                                        &mut writer_failure,
                                        &read_buffer,
                                        &mut rx,
                                        &mut pending_messages,
                                        &pending_terminate,
                                        control,
                                    ).await? {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        changed = epoch_latch.changed() => {
                            if changed.is_err() {
                                return Err(RuntimeError::Config(
                                    "runtime epoch latch closed while a session was active".to_string(),
                                ));
                            }
                            let notified_revision = *epoch_latch.borrow_and_update();
                            let pending_revision = match &lifecycle {
                                SessionLifecycle::TransferFrozen { pending, .. } => {
                                    pending.epoch_revision
                                }
                                _ => unreachable!(
                                    "epoch latch is only awaited by a transfer-frozen session"
                                ),
                            };
                            if notified_revision != pending_revision {
                                continue;
                            }
                            let activation = self.activate_committed_cutover(
                                &mut lifecycle,
                                &directory,
                                &mut transport_io,
                            );
                            if let Some(activation_tx) = imported_activation.take() {
                                let acknowledged = activation
                                    .as_ref()
                                    .map(|()| ())
                                    .map_err(ToString::to_string);
                                let _ = activation_tx.send(acknowledged);
                            }
                            activation?;
                        }
                        changed = writer_failure.changed() => {
                            if changed.is_err() {
                                return Err(RuntimeError::Io(std::io::Error::new(
                                    std::io::ErrorKind::BrokenPipe,
                                    "transport writer stopped while the session was active",
                                )));
                            }
                            if let Some(error) = writer_failure.borrow().clone() {
                                return Err(RuntimeError::Io(std::io::Error::other(error)));
                            }
                        }
                    }
                    continue;
                }

                let data_plane = loop {
                    tokio::select! {
                        biased;
                        control = control_rx.recv() => {
                            let Some(control) = control else {
                                break 'session;
                            };
                            match control {
                                control @ SessionControl::Terminate { .. } => {
                                    pending_terminate = Some(control);
                                }
                                control @ SessionControl::EnforceGenerationDrain => {
                                    pending_terminate.get_or_insert(control);
                                }
                                control => {
                                    if self.handle_session_control(
                                        connection_id,
                                        &mut transport_io,
                                        &mut lifecycle,
                                        &directory,
                                        &mut process_transfer,
                                        &mut process_transfer_slot,
                                        &mut writer_failure,
                                        &read_buffer,
                                        &mut rx,
                                        &mut pending_messages,
                                        &pending_terminate,
                                        control,
                                    ).await? {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        permit = self.authority.enter_data_plane() => break permit,
                    }
                };
                let activation =
                    self.activate_committed_cutover(&mut lifecycle, &directory, &mut transport_io);
                if let Some(activation_tx) = imported_activation.take() {
                    let acknowledged = activation
                        .as_ref()
                        .map(|()| ())
                        .map_err(ToString::to_string);
                    let _ = activation_tx.send(acknowledged);
                }
                activation?;
                if let Some(control) = pending_terminate.take()
                    && self
                        .handle_session_control(
                            connection_id,
                            &mut transport_io,
                            &mut lifecycle,
                            &directory,
                            &mut process_transfer,
                            &mut process_transfer_slot,
                            &mut writer_failure,
                            &read_buffer,
                            &mut rx,
                            &mut pending_messages,
                            &pending_terminate,
                            control,
                        )
                        .await?
                {
                    return Ok(());
                }
                if !read_buffer.is_empty() {
                    if self
                        .process_read_buffer(
                            connection_id,
                            &mut transport_io,
                            &mut lifecycle,
                            &directory,
                            &mut read_buffer,
                        )
                        .await?
                    {
                        return Ok(());
                    }
                    // A remaining prefix needs another transport read. Re-entering this loop
                    // without new bytes would hold the data-plane gate forever on a split frame.
                }

                if let Some(message) = pending_messages.pop_front() {
                    let phase = Self::running_phase_mut(&mut lifecycle)?;
                    let should_close = self
                        .handle_outgoing_message(connection_id, &mut transport_io, phase, message)
                        .await?;
                    Self::publish_directory(phase, &directory);
                    if should_close {
                        return Ok(());
                    }
                    continue;
                }
                drop(data_plane);

                tokio::select! {
                    biased;
                    changed = writer_failure.changed() => {
                        if changed.is_err() {
                            return Err(RuntimeError::Io(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "transport writer stopped while the session was active",
                            )));
                        }
                        if let Some(error) = writer_failure.borrow().clone() {
                            return Err(RuntimeError::Io(std::io::Error::other(error)));
                        }
                    }
                    Some(control) = control_rx.recv() => {
                        match control {
                            control @ SessionControl::Terminate { .. } => {
                                pending_terminate = Some(control);
                            }
                            control @ SessionControl::EnforceGenerationDrain => {
                                pending_terminate.get_or_insert(control);
                            }
                            control => {
                                if self.handle_session_control(
                                    connection_id,
                                    &mut transport_io,
                                    &mut lifecycle,
                                    &directory,
                                    &mut process_transfer,
                                    &mut process_transfer_slot,
                                    &mut writer_failure,
                                    &read_buffer,
                                    &mut rx,
                                    &mut pending_messages,
                                    &pending_terminate,
                                    control,
                                ).await? {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    read = transport_io.read_into(&mut read_buffer) => {
                        if read? == 0 {
                            break;
                        }
                    }
                    maybe_message = rx.recv() => {
                        let Some(message) = maybe_message else {
                            break;
                        };
                        pending_messages.push_back(message);
                    }
                }
            }
            Ok(())
        }
        .await;

        transport_io.shutdown_writer().await;

        let cleanup = match lifecycle {
            SessionLifecycle::Running { phase, .. } => {
                Self::publish_directory(
                    &SessionPhase::Closing {
                        generation_id: phase.generation_id(),
                        transport: phase.transport(),
                        phase: phase.phase(),
                    },
                    &directory,
                );
                self.unregister_session(connection_id, &phase).await
            }
            SessionLifecycle::CutoverPrepared { active, .. } => {
                Self::publish_directory(
                    &SessionPhase::Closing {
                        generation_id: active.generation_id(),
                        transport: active.transport(),
                        phase: active.phase(),
                    },
                    &directory,
                );
                self.unregister_session(connection_id, &active).await
            }
            SessionLifecycle::TransferFrozen { active, .. } => {
                Self::publish_directory(
                    &SessionPhase::Closing {
                        generation_id: active.generation_id(),
                        transport: active.transport(),
                        phase: active.phase(),
                    },
                    &directory,
                );
                self.unregister_session(connection_id, &active).await
            }
            SessionLifecycle::Transferred => {
                self.sessions.remove(connection_id).await;
                Ok(())
            }
        };
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => Err(RuntimeError::Config(format!(
                "session {connection_id:?} ended with error: {error}; cleanup failed: {cleanup_error}"
            ))),
        }
    }

    pub(in crate::runtime) async fn reap_completed_session_tasks(&self) {
        self.sessions.reap_completed_tasks().await;
    }

    pub(in crate::runtime) async fn join_all_session_tasks(&self) {
        self.sessions.join_all_tasks().await;
    }

    async fn handle_session_control(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        lifecycle: &mut SessionLifecycle,
        directory: &SessionDirectoryProjection,
        process_transfer: &mut Option<FrozenProcessTransport>,
        process_transfer_slot: &mut Option<ArenaReservation>,
        writer_failure: &mut tokio::sync::watch::Receiver<Option<String>>,
        read_buffer: &BytesMut,
        rx: &mut mpsc::Receiver<SessionMessage>,
        pending_messages: &mut VecDeque<SessionMessage>,
        pending_terminate: &Option<SessionControl>,
        control: SessionControl,
    ) -> Result<bool, RuntimeError> {
        match control {
            SessionControl::EnforceGenerationDrain => {
                // Deferred alongside termination until data-plane admission and latch
                // activation. A stale registry notification cannot retire a new binding.
                let phase = Self::running_phase_mut(lifecycle)?;
                match self.generation_admission(phase.generation_id()) {
                    GenerationAdmission::Active(_) | GenerationAdmission::Draining(_) => Ok(false),
                    GenerationAdmission::ExpiredDraining | GenerationAdmission::Missing => {
                        let should_close = self
                            .handle_outgoing_message(
                                connection_id,
                                transport_io,
                                phase,
                                SessionMessage::Terminate {
                                    reason: "Server generation reloaded".to_string(),
                                },
                            )
                            .await?;
                        Self::publish_directory(phase, directory);
                        Ok(should_close)
                    }
                }
            }
            SessionControl::Terminate { reason } => {
                let phase = Self::running_phase_mut(lifecycle)?;
                let should_close = self
                    .handle_outgoing_message(
                        connection_id,
                        transport_io,
                        phase,
                        SessionMessage::Terminate { reason },
                    )
                    .await?;
                Self::publish_directory(phase, directory);
                Ok(should_close)
            }
            SessionControl::PrepareCutover {
                candidate,
                resync_events,
                force_resync,
                ack_tx,
            } => {
                let result =
                    match self.activate_committed_cutover(lifecycle, directory, transport_io) {
                        Ok(()) => self.prepare_session_cutover(
                            connection_id,
                            lifecycle,
                            candidate,
                            resync_events,
                            force_resync,
                        ),
                        Err(error) => Err(error),
                    };
                let _ = ack_tx.send(result);
                Ok(false)
            }
            SessionControl::SealCutover {
                epoch_revision,
                final_events,
                ack_tx,
            } => {
                let result = match transport_io.pause_writer().await {
                    Ok(()) => match transport_io.freeze_transport().await {
                        Ok(()) => self.seal_session_cutover(
                            connection_id,
                            lifecycle,
                            epoch_revision,
                            final_events,
                        ),
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                };
                let _ = ack_tx.send(result);
                Ok(false)
            }
            SessionControl::AbortCutover {
                epoch_revision,
                ack_tx,
            } => {
                let current = std::mem::replace(lifecycle, SessionLifecycle::Transferred);
                let result = match current {
                    SessionLifecycle::CutoverPrepared {
                        active_epoch_revision,
                        active,
                        pending,
                    }
                    | SessionLifecycle::TransferFrozen {
                        active_epoch_revision,
                        active,
                        pending,
                    } if pending.epoch_revision == epoch_revision => {
                        *lifecycle = SessionLifecycle::Running {
                            epoch_revision: active_epoch_revision,
                            phase: active,
                        };
                        let transport_result = transport_io.resume_transport();
                        let writer_result = transport_io.resume_writer_with_front(Vec::new());
                        match (transport_result, writer_result) {
                            (Ok(()), Ok(())) => Ok(()),
                            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                            (Err(transport_error), Err(writer_error)) => {
                                Err(RuntimeError::Config(format!(
                                    "transport resume failed: {transport_error}; writer resume failed: {writer_error}"
                                )))
                            }
                        }
                    }
                    other => {
                        *lifecycle = other;
                        Ok(())
                    }
                };
                let transport_revoked = result.is_err();
                let _ = ack_tx.send(result);
                Ok(transport_revoked)
            }
            SessionControl::PrepareProcessTransferSocket {
                target,
                arena,
                ack_tx,
            } => {
                let result = self
                    .activate_committed_cutover(lifecycle, directory, transport_io)
                    .and_then(|()| {
                        if process_transfer.is_some() {
                            return Err(RuntimeError::Config(format!(
                                "session {connection_id:?} already holds frozen process-transfer authority"
                            )));
                        }
                        if process_transfer_slot.is_some() {
                            return Err(RuntimeError::Config(format!(
                                "session {connection_id:?} already owns a prepared process-transfer slot"
                            )));
                        }
                        if !matches!(lifecycle, SessionLifecycle::Running { .. }) {
                            return Err(RuntimeError::Config(format!(
                                "session {connection_id:?} cannot prepare process transfer from its current lifecycle"
                            )));
                        } else {
                            let phase = match lifecycle {
                                SessionLifecycle::Running { phase, .. } => phase,
                                _ => unreachable!("running lifecycle was checked above"),
                            };
                            executable_session_slot_capacity(phase)
                                .and_then(|capacity| {
                                    arena
                                        .reserve(capacity)
                                        .map_err(|error| RuntimeError::Config(error.to_string()))
                                })
                                .and_then(|slot| {
                                    transport_io
                                        .duplicate_socket_for_process_transfer(target)
                                        .map(|socket| {
                                            *process_transfer_slot = Some(slot);
                                            socket
                                        })
                                })
                        }
                    });
                let _ = ack_tx.send(result);
                Ok(false)
            }
            SessionControl::FreezeForProcessTransfer { ack_tx } => {
                let result = if process_transfer.is_some() {
                    Err(RuntimeError::Config(format!(
                        "session {connection_id:?} is already frozen for process transfer"
                    )))
                } else if !matches!(lifecycle, SessionLifecycle::Running { .. }) {
                    Err(RuntimeError::Config(format!(
                        "session {connection_id:?} cannot enter process transfer from its current lifecycle"
                    )))
                } else {
                    let Some(slot) = process_transfer_slot.take() else {
                        let _ = ack_tx.send(Err(RuntimeError::Config(format!(
                            "session {connection_id:?} has no prepared process-transfer slot"
                        ))));
                        return Ok(false);
                    };
                    loop {
                        match rx.try_recv() {
                            Ok(message) => pending_messages.push_back(message),
                            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                        }
                    }
                    let pending_terminate_reason =
                        pending_terminate
                            .as_ref()
                            .and_then(|control| match control {
                                SessionControl::Terminate { reason } => Some(reason.as_str()),
                                // The child owns its topology deadlines and periodic drain
                                // checks. A registry wakeup is not transferable termination
                                // authority; the parent retains it if transfer aborts.
                                SessionControl::EnforceGenerationDrain => None,
                                _ => None,
                            });
                    let (epoch_revision, phase) = match lifecycle {
                        SessionLifecycle::Running {
                            epoch_revision,
                            phase,
                        } => (*epoch_revision, phase),
                        _ => unreachable!("running lifecycle was checked above"),
                    };
                    match transport_io.freeze_for_process_transfer().await {
                        Ok(frozen) => {
                            match capture_executable_session(
                                connection_id,
                                epoch_revision,
                                phase,
                                read_buffer,
                                pending_messages,
                                pending_terminate_reason,
                            ) {
                                Ok(actor) => {
                                    match executable_session_state(actor, frozen.transfer_state())
                                        .and_then(|state| {
                                            state.write_to_slot(slot).map(|region| (state, region))
                                        }) {
                                        Ok((state, region)) => {
                                            *process_transfer = Some(frozen);
                                            Ok(FrozenProcessSessionState { state, region })
                                        }
                                        Err(error) => match frozen.rollback().await {
                                            Ok(running) => {
                                                *transport_io = running;
                                                *writer_failure =
                                                    transport_io.subscribe_writer_failure();
                                                Err(error)
                                            }
                                            Err(rollback) => Err(RuntimeError::Config(format!(
                                                "{error}; process-transfer transport rollback failed: {rollback}"
                                            ))),
                                        },
                                    }
                                }
                                Err(error) => match frozen.rollback().await {
                                    Ok(running) => {
                                        *transport_io = running;
                                        *writer_failure = transport_io.subscribe_writer_failure();
                                        Err(error)
                                    }
                                    Err(rollback) => Err(RuntimeError::Config(format!(
                                        "{error}; process-transfer transport rollback failed: {rollback}"
                                    ))),
                                },
                            }
                        }
                        Err(error) => Err(error),
                    }
                };
                let _ = ack_tx.send(result);
                Ok(false)
            }
            SessionControl::RollbackProcessTransfer { ack_tx } => {
                process_transfer_slot.take();
                let result = match process_transfer.take() {
                    Some(frozen) => frozen.rollback().await.map(|running| {
                        *transport_io = running;
                    }),
                    None => Ok(()),
                };
                if result.is_ok() {
                    *writer_failure = transport_io.subscribe_writer_failure();
                }
                let transport_revoked = result.is_err();
                let _ = ack_tx.send(result);
                Ok(transport_revoked)
            }
            SessionControl::CommitProcessTransfer { ack_tx } => {
                let result = match process_transfer.take() {
                    Some(frozen) => frozen.commit().await,
                    None => Err(RuntimeError::Config(format!(
                        "session {connection_id:?} has no frozen process-transfer authority"
                    ))),
                };
                let committed = result.is_ok();
                if committed {
                    *lifecycle = SessionLifecycle::Transferred;
                }
                let _ = ack_tx.send(result);
                Ok(committed)
            }
        }
    }

    fn prepare_session_cutover(
        &self,
        connection_id: ConnectionId,
        lifecycle: &mut SessionLifecycle,
        candidate: Arc<crate::runtime::RuntimeEpoch>,
        resync_events: Vec<Arc<revy_voxel_core::CoreEvent>>,
        force_resync: bool,
    ) -> Result<(), RuntimeError> {
        let current = std::mem::replace(lifecycle, SessionLifecycle::Transferred);
        let (active_epoch_revision, active) = match current {
            SessionLifecycle::Running {
                epoch_revision,
                phase,
            } => (epoch_revision, phase),
            SessionLifecycle::CutoverPrepared {
                active_epoch_revision,
                active,
                pending,
            } if pending.epoch_revision == candidate.revision() => (active_epoch_revision, active),
            frozen @ SessionLifecycle::TransferFrozen { .. } => {
                *lifecycle = frozen;
                return Err(RuntimeError::Config(format!(
                    "session {connection_id:?} cannot prepare while its transport is frozen"
                )));
            }
            other => {
                *lifecycle = other;
                return Err(RuntimeError::Config(format!(
                    "session {connection_id:?} cannot prepare the requested cutover"
                )));
            }
        };
        let pending = (|| {
            let pending_phase = self.build_candidate_session_phase(&active, &candidate)?;
            let mut resync_frames = Vec::new();
            if let SessionPhase::Play(play) = &pending_phase
                && (force_resync || Self::binding_changed(&active, &pending_phase))
            {
                let snapshot = pending_phase.protocol_snapshot(connection_id);
                for event in resync_events {
                    let packets = play.binding.adapter.encode_play_event(
                        event.as_ref(),
                        &snapshot,
                        &mc_proto_common::PlayEncodingContext {
                            player_id: play.player_id,
                            entity_id: play.entity_id,
                        },
                    )?;
                    for packet in packets {
                        resync_frames
                            .push(play.binding.adapter.wire_codec().encode_frame(&packet)?);
                    }
                }
            }
            // Validate the candidate's handoff contract while the data plane is open.
            // Seal refreshes this state after the last active invocation has completed.
            Self::transfer_candidate_plugin_state(connection_id, &active, &pending_phase)?;
            Ok::<_, RuntimeError>(PendingSessionState {
                epoch_revision: candidate.revision(),
                phase: pending_phase,
                resync_frames,
            })
        })();
        match pending {
            Ok(pending) => {
                *lifecycle = SessionLifecycle::CutoverPrepared {
                    active_epoch_revision,
                    active,
                    pending,
                };
                Ok(())
            }
            Err(error) => {
                *lifecycle = SessionLifecycle::Running {
                    epoch_revision: active_epoch_revision,
                    phase: active,
                };
                Err(error)
            }
        }
    }

    fn binding_changed(active: &SessionPhase, pending: &SessionPhase) -> bool {
        match (active, pending) {
            (SessionPhase::Play(active), SessionPhase::Play(pending)) => {
                active.binding.capabilities.protocol_generation
                    != pending.binding.capabilities.protocol_generation
                    || active.binding.capabilities.gameplay_generation
                        != pending.binding.capabilities.gameplay_generation
            }
            _ => false,
        }
    }

    fn build_candidate_session_phase(
        &self,
        active: &SessionPhase,
        candidate: &crate::runtime::RuntimeEpoch,
    ) -> Result<SessionPhase, RuntimeError> {
        let candidate_binding = |binding: &BoundSession, entity_id| {
            let adapter_id = binding.adapter.descriptor().adapter_id;
            let adapter = candidate
                .topology
                .protocol_registry
                .resolve_adapter(&adapter_id)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "candidate epoch is missing adapter `{adapter_id}`"
                    ))
                })?;
            if adapter.descriptor().transport != binding.transport {
                return Err(RuntimeError::Config(format!(
                    "candidate adapter `{adapter_id}` changed transport"
                )));
            }
            let profile_id =
                crate::runtime::selection::SelectionResolver::gameplay_profile_for_adapter(
                    &candidate.selection.config,
                    &adapter_id,
                );
            let gameplay = candidate
                .selection
                .loaded_plugins
                .resolve_gameplay_profile(profile_id.as_str())
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "candidate epoch is missing gameplay profile `{}`",
                        profile_id.as_str()
                    ))
                })?;
            Ok(BoundSession::new(
                Arc::clone(&candidate.topology),
                binding.transport,
                adapter,
                gameplay,
                entity_id,
            ))
        };
        Ok(match active {
            SessionPhase::Handshaking { .. } => SessionPhase::Handshaking {
                generation: Arc::clone(&candidate.topology),
            },
            SessionPhase::Status(binding) => {
                SessionPhase::Status(candidate_binding(binding, None)?)
            }
            SessionPhase::Login(LoginSession::Negotiating(binding)) => {
                SessionPhase::Login(LoginSession::Negotiating(candidate_binding(binding, None)?))
            }
            SessionPhase::Login(LoginSession::Authenticating { binding, challenge }) => {
                SessionPhase::Login(LoginSession::Authenticating {
                    binding: candidate_binding(binding, None)?,
                    challenge: challenge.clone(),
                })
            }
            SessionPhase::Login(LoginSession::AcceptedWritePending {
                binding,
                player_id,
                entity_id,
            }) => SessionPhase::Login(LoginSession::AcceptedWritePending {
                binding: candidate_binding(binding, Some(*entity_id))?,
                player_id: *player_id,
                entity_id: *entity_id,
            }),
            SessionPhase::Play(play) => SessionPhase::Play(PlaySession {
                binding: candidate_binding(&play.binding, Some(play.entity_id))?,
                player_id: play.player_id,
                entity_id: play.entity_id,
            }),
            SessionPhase::Closing {
                generation_id: _,
                transport,
                phase,
            } => SessionPhase::Closing {
                generation_id: candidate.topology.generation_id,
                transport: *transport,
                phase: *phase,
            },
        })
    }

    fn seal_session_cutover(
        &self,
        connection_id: ConnectionId,
        lifecycle: &mut SessionLifecycle,
        epoch_revision: u64,
        final_events: Vec<Arc<revy_voxel_core::CoreEvent>>,
    ) -> Result<(), RuntimeError> {
        let current = std::mem::replace(lifecycle, SessionLifecycle::Transferred);
        let (active_epoch_revision, active, pending) = match current {
            SessionLifecycle::CutoverPrepared {
                active_epoch_revision,
                active,
                pending,
            } => (active_epoch_revision, active, pending),
            other => {
                *lifecycle = other;
                return Err(RuntimeError::Config(format!(
                    "session {connection_id:?} was not prepared before cutover seal"
                )));
            }
        };
        *lifecycle = SessionLifecycle::TransferFrozen {
            active_epoch_revision,
            active,
            pending,
        };
        let SessionLifecycle::TransferFrozen {
            active, pending, ..
        } = lifecycle
        else {
            unreachable!("sealed session lifecycle was just installed")
        };
        if pending.epoch_revision != epoch_revision {
            return Err(RuntimeError::Config(format!(
                "session {connection_id:?} prepared epoch {} but cutover {} was sealed",
                pending.epoch_revision, epoch_revision
            )));
        }
        Self::transfer_candidate_plugin_state(connection_id, active, &pending.phase)?;
        let SessionPhase::Play(play) = &pending.phase else {
            return Ok(());
        };
        let snapshot = pending.phase.protocol_snapshot(connection_id);
        for event in final_events {
            let packets = play.binding.adapter.encode_play_event(
                event.as_ref(),
                &snapshot,
                &mc_proto_common::PlayEncodingContext {
                    player_id: play.player_id,
                    entity_id: play.entity_id,
                },
            )?;
            for packet in packets {
                pending
                    .resync_frames
                    .push(play.binding.adapter.wire_codec().encode_frame(&packet)?);
            }
        }
        Ok(())
    }

    /// Copies plugin-owned state only into the inactive object. Equal artifact/binding
    /// hashes do not imply object identity. Final events are encoded after the handoff
    /// so their protocol state transitions follow the snapshot, just as their frames do.
    fn transfer_candidate_plugin_state(
        connection_id: ConnectionId,
        active: &SessionPhase,
        pending: &SessionPhase,
    ) -> Result<(), RuntimeError> {
        let (Some(source), Some(destination)) = (active.binding(), pending.binding()) else {
            return Ok(());
        };
        if !Arc::ptr_eq(&source.adapter, &destination.adapter) {
            let blob = source
                .adapter
                .export_session_state(&active.protocol_snapshot(connection_id))?;
            destination
                .adapter
                .import_session_state(&pending.protocol_snapshot(connection_id), &blob)?;
        }
        if !Arc::ptr_eq(&source.gameplay, &destination.gameplay)
            && let (Some(source_session), Some(destination_session)) =
                (active.gameplay_snapshot(), pending.gameplay_snapshot())
        {
            let blob = source.gameplay.export_session_state(&source_session)?;
            destination
                .gameplay
                .import_session_state(&destination_session, &blob)?;
        }
        Ok(())
    }

    fn activate_committed_cutover(
        &self,
        lifecycle: &mut SessionLifecycle,
        directory: &SessionDirectoryProjection,
        transport_io: &mut TransportSessionIo,
    ) -> Result<(), RuntimeError> {
        let active_revision = self.authority.active().revision();
        let current = std::mem::replace(lifecycle, SessionLifecycle::Transferred);
        match current {
            SessionLifecycle::CutoverPrepared {
                active_epoch_revision,
                active,
                pending,
            } => {
                let prepared_revision = pending.epoch_revision;
                *lifecycle = SessionLifecycle::CutoverPrepared {
                    active_epoch_revision,
                    active,
                    pending,
                };
                if prepared_revision == active_revision {
                    return Err(RuntimeError::Config(
                        "committed session transport was not frozen before activation".to_string(),
                    ));
                }
                if active_epoch_revision == active_revision {
                    return Ok(());
                }
                return Err(RuntimeError::Config(format!(
                    "session lifecycle did not contain pending state for committed epoch {active_revision}"
                )));
            }
            SessionLifecycle::TransferFrozen {
                active_epoch_revision: _,
                active: _,
                pending,
            } if pending.epoch_revision == active_revision => {
                let PendingSessionState {
                    epoch_revision: _,
                    phase,
                    resync_frames,
                } = pending;
                Self::publish_directory(&phase, directory);
                *lifecycle = SessionLifecycle::Running {
                    epoch_revision: active_revision,
                    phase,
                };
                transport_io.resume_transport()?;
                transport_io.resume_writer_with_front(resync_frames)?;
            }
            SessionLifecycle::Running {
                epoch_revision,
                phase,
            } if epoch_revision == active_revision => {
                *lifecycle = SessionLifecycle::Running {
                    epoch_revision,
                    phase,
                };
                transport_io.resume_writer_with_front(Vec::new())?;
            }
            stale
            @ (SessionLifecycle::Running { .. } | SessionLifecycle::TransferFrozen { .. }) => {
                *lifecycle = stale;
                return Err(RuntimeError::Config(format!(
                    "session lifecycle did not contain pending state for committed epoch {active_revision}"
                )));
            }
            SessionLifecycle::Transferred => {
                return Err(RuntimeError::Config(
                    "transferred session cannot activate a runtime epoch".to_string(),
                ));
            }
        }
        Ok(())
    }
}
