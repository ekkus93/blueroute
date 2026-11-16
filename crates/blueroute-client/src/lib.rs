#![doc = "Reusable client for the BlueRoute daemon API."]

mod watch;
pub use watch::{WatchSignal, reconnect_delay, watch};

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::NodeCapabilities;
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    Event, ProtocolCodecError, Response, STATE_CHANGED_SIGNAL, decode_event, decode_response,
    encode_command,
};
use futures_lite::StreamExt;
use zbus::fdo::{DBusProxy, NameOwnerChangedStream};
use zbus::message::Sequence;
use zbus::names::OwnedUniqueName;
use zbus::proxy::SignalStream;
use zbus::{Connection, Proxy};

const RECONNECT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Reusable version-gated client for the local BlueRoute daemon.
///
/// Every operation resolves the daemon's well-known name to its unique bus name once, checks the
/// version there, and talks only to that unique name (CR-F8, OWN-1..OWN-4). A replacement owner
/// that takes the name mid-operation is never reached; an owner that disappears makes the
/// operation fail as unavailable. Nothing is ever retried automatically.
#[derive(Clone)]
pub struct BlueRouteClient {
    connection: Connection,
    /// The version of the owner most recently validated. Informational only: every operation
    /// validates its own owner again.
    server_version: ApiVersion,
}

/// A daemon owner whose compatible version was checked at its unique bus name.
struct ValidatedOwner {
    name: OwnedUniqueName,
    version: ApiVersion,
    /// Receive position of the `GetNameOwner` reply that named this owner: the start of the
    /// ownership period it was validated in.
    since: Sequence,
}

impl BlueRouteClient {
    /// Connect to the production system bus and reject an incompatible daemon before returning.
    pub async fn connect() -> Result<Self, ClientError> {
        let connection = Connection::system().await.map_err(ClientError::Bus)?;
        Self::from_connection(connection).await
    }

    /// Build a client over an established bus connection.
    ///
    /// This is useful for deterministic tests and callers that already own a zbus connection.
    pub async fn from_connection(connection: Connection) -> Result<Self, ClientError> {
        let owner = validated_owner(&connection).await?;
        Ok(Self {
            connection,
            server_version: owner.version,
        })
    }

    pub const fn server_version(&self) -> ApiVersion {
        self.server_version
    }

    /// Wait for a compatible daemon to become available again after a restart.
    ///
    /// Only the read-only owner lookup and `Version` negotiation are retried, after a bus failure.
    /// Normal commands are never replayed automatically because retrying a mutation after a lost
    /// reply could duplicate side effects. An incompatible daemon fails at once, without retries.
    ///
    /// `timeout` bounds the whole operation (RQR-01, DEADLINE-1..6). One deadline, taken when the
    /// returned future is first polled, covers every owner lookup, Version negotiation and retry
    /// delay; delays are cut to the time left and nothing new is sent once it has passed. An
    /// outcome counts only if it is decided before the deadline: when a reply and the deadline
    /// are both ready, including a reply that arrived while the future was not being polled, the
    /// deadline wins and the result is [`ClientError::ReconnectTimeout`], whether that reply was
    /// compatible or not. A zero `timeout` returns that error without contacting the bus.
    ///
    /// [`server_version`](Self::server_version) changes only on a timely compatible reply. An
    /// expired or dropped reconnect leaves it unchanged and abandons its pending read-only call
    /// without closing the connection, which stays usable for later calls and subscriptions.
    /// As with any cancelled zbus call, the one transport-level exception is a call abandoned in
    /// the middle of writing its message to the socket, which zbus does not guard against.
    pub async fn reconnect(&mut self, timeout: Duration) -> Result<(), ClientError> {
        let connection = &self.connection;
        let version = negotiate_within(timeout, || async {
            validated_owner(connection).await.map(|owner| owner.version)
        })
        .await?;
        self.server_version = version;
        Ok(())
    }

    /// Send one typed protocol command.
    ///
    /// The command goes to exactly the owner whose compatible version was checked immediately
    /// before, addressed by its unique bus name. A daemon replaced in between never receives it,
    /// and a reply lost in transit is reported, never answered by resending the command.
    pub async fn request(&self, command: &Command) -> Result<Response, ClientError> {
        let owner = validated_owner(&self.connection).await?;
        let payload = encode_command(command).map_err(ClientError::Protocol)?;
        let proxy = owner_proxy(&self.connection, &owner.name).await?;
        let response_payload: String = proxy
            .call("Request", &(payload.as_str(),))
            .await
            .map_err(ClientError::Bus)?;
        decode_response(&response_payload).map_err(ClientError::Protocol)
    }

    pub async fn status(&self) -> Result<DaemonStatus, ClientError> {
        match self.request(&Command::GetStatus).await? {
            Response::Status(status) => Ok(status),
            other => Err(ClientError::UnexpectedResponse {
                expected: "status",
                actual: response_kind(&other),
            }),
        }
    }

    pub async fn capabilities(&self) -> Result<NodeCapabilities, ClientError> {
        match self.request(&Command::GetCapabilities).await? {
            Response::Capabilities(capabilities) => Ok(capabilities),
            other => Err(ClientError::UnexpectedResponse {
                expected: "capabilities",
                actual: response_kind(&other),
            }),
        }
    }

    /// Subscribe to typed daemon events from the owner whose compatible version was just checked.
    /// Signals are matched on that owner's unique bus name, so a replacement owner's events never
    /// arrive on this subscription. The subscription also watches the name itself and ends with
    /// [`ClientError::OwnerLost`] when that owner gives it up or disappears, even if the daemon
    /// never emits another event (CR-F2, WATCH-1).
    pub async fn events(&self) -> Result<EventSubscription, ClientError> {
        self.subscribe(false).await
    }

    /// Like [`Self::events`], but also delivers the daemon's `StateChanged` invalidations
    /// (API 1.7) as [`Change::State`] through [`EventSubscription::next_change`]. This is what a
    /// live view needs: any change it could display, not only typed events (CR-F1).
    pub async fn changes(&self) -> Result<EventSubscription, ClientError> {
        self.subscribe(true).await
    }

    async fn subscribe(&self, with_state: bool) -> Result<EventSubscription, ClientError> {
        // Watch the name before resolving its owner, so no ownership change can fall between
        // the two steps unobserved.
        let owner_changes = DBusProxy::new(&self.connection)
            .await
            .map_err(ClientError::Bus)?
            .receive_name_owner_changed_with_args(&[(0, DBUS_SERVICE_NAME)])
            .await
            .map_err(ClientError::Bus)?;
        let owner = validated_owner(&self.connection).await?;
        let proxy = owner_proxy(&self.connection, &owner.name).await?;
        let stream = proxy
            .receive_signal("Event")
            .await
            .map_err(ClientError::Bus)?;
        let state = if with_state {
            Some(
                proxy
                    .receive_signal(STATE_CHANGED_SIGNAL)
                    .await
                    .map_err(ClientError::Bus)?,
            )
        } else {
            None
        };
        Ok(EventSubscription {
            connection: self.connection.clone(),
            owner: owner.name,
            since: owner.since,
            live: Some(Receivers {
                events: stream,
                state,
                owner_changes,
            }),
            backlog: VecDeque::new(),
            period_end: None,
            ended: None,
        })
    }
}

/// One thing a [`BlueRouteClient::changes`] subscription reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A typed daemon event.
    Event(Event),
    /// Something observable changed; reread. The revision only orders signals of one daemon
    /// process and is not a state version (see `blueroute_protocol::STATE_CHANGED_SIGNAL`).
    State { revision: u64 },
}

/// The most items a subscription holds that it has received but not yet delivered. Beyond it the
/// subscription ends with [`ClientError::SubscriptionLagged`] instead of stalling its connection
/// (QR-QUEUE, QUEUE-4).
pub const SUBSCRIPTION_BACKLOG: usize = 256;

/// Event stream that rechecks daemon compatibility before accepting every event payload.
///
/// A subscription belongs to one continuous ownership period of the daemon's name: the one in
/// which `owner` was validated, starting at `since`. Once that owner gives the name up, the
/// period is over for good; the same connection taking the name back starts a new period that
/// only a fresh subscription can bind to (LC-OWNER, EPOCH-1).
///
/// # Queues and progress (QR-QUEUE, QUEUE-1/4)
///
/// The subscription owns three zbus match streams: the bound owner's `Event` signals, its
/// `StateChanged` signals (for [`BlueRouteClient::changes`]), and the ownership notices of the
/// daemon's name. zbus 5.19 reads a connection's messages one at a time and waits for room in
/// every matching stream (64 entries each by default) before reading the next, so a stream that
/// is not read stops every reply on the connection, including this subscription's own checks.
/// Therefore no await inside [`Self::next_change`] is left without reading all three: while a
/// signal, a Version reply or the final `GetNameOwner` reply is awaited, notices are settled as
/// they arrive and signals move into a backlog of at most [`SUBSCRIPTION_BACKLOG`] items. A
/// consumer that falls further behind gets [`ClientError::SubscriptionLagged`] and should reread
/// state. Retirement and lag release all three streams, so a handle kept alive afterwards holds
/// nothing back; that includes a retirement found after the decision, which ends the
/// subscription as its last item is delivered. Dropping the subscription releases them too. One
/// case waits for the next call or a drop: a call cancelled after it read a period end, which
/// only that next call can report. Between calls the streams are not read: a caller that pauses
/// for long can stop its connection's reader until it resumes.
pub struct EventSubscription {
    connection: Connection,
    /// The unique bus name the subscription is bound to.
    owner: OwnedUniqueName,
    /// Where the bound ownership period began, in this connection's receive order. Ownership
    /// notices received before it describe earlier history and are ignored.
    since: Sequence,
    /// The match streams; `None` once the subscription has ended.
    live: Option<Receivers>,
    /// Signals received but not yet delivered, in receive order. The front one is the item being
    /// checked; it leaves only when delivered, so a cancelled call loses nothing.
    backlog: VecDeque<zbus::Message>,
    /// The receive position of a period-ending notice read while a final verification was
    /// pending. It is kept here, not in the call, so cancelling that call cannot lose it.
    period_end: Option<Sequence>,
    /// Set once the subscription has ended; every later call reports the same error.
    ended: Option<Ended>,
}

/// The three match streams a live subscription reads.
struct Receivers {
    events: SignalStream<'static>,
    /// `StateChanged` invalidations, for a [`BlueRouteClient::changes`] subscription.
    state: Option<SignalStream<'static>>,
    /// Ownership changes of the daemon's well-known name only.
    owner_changes: NameOwnerChangedStream,
}

/// Why a subscription ended.
#[derive(Clone, Copy)]
enum Ended {
    OwnerLost,
    Lagged,
}

impl Ended {
    fn error(self) -> ClientError {
        match self {
            Self::OwnerLost => ClientError::OwnerLost,
            Self::Lagged => ClientError::SubscriptionLagged {
                limit: SUBSCRIPTION_BACKLOG,
            },
        }
    }
}

/// Whether one ownership notice ends `owner`'s period that began at `since`: received after
/// that start, it leaves the name with anyone but `owner` (another owner, or nobody). A notice
/// that leaves the name with `owner` (its own acquisition) does not, and neither does one from
/// before the period began. An unreadable notice ends it: ownership can no longer be proven.
fn ends_period(
    change: &zbus::fdo::NameOwnerChanged,
    owner: &OwnedUniqueName,
    since: Sequence,
) -> bool {
    if change.message().recv_position() <= since {
        return false;
    }
    change
        .args()
        .ok()
        .and_then(|args| args.new_owner().as_ref().map(|new| new == owner))
        != Some(true)
}

/// Whether a failed final ownership check demonstrates owner loss. Only "nobody owns the name"
/// does. A transient failure of the check itself (a bus timeout, for instance) is reported
/// without ending a healthy subscription (EPOCH-3).
fn shows_no_owner(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Bus(zbus::Error::FDO(error))
            if matches!(**error, zbus::fdo::Error::NameHasNoOwner(_))
    )
}

/// What a final verification decided about the item in hand (QUEUE-3, EPOCH-2).
#[derive(Debug, PartialEq)]
enum Decision {
    /// Deliver it; the period is still running.
    Deliver,
    /// Deliver it, but its period ended after the decision: every later call reports OwnerLost.
    DeliverThenRetire,
    /// The period ended before the decision: do not deliver.
    Retire,
}

/// The delivery decision. `decision` is the receive position of the final `GetNameOwner` reply
/// naming the bound owner; `period_end` is the earliest period-ending notice read so far, if any.
/// A notice received before the reply ends the period before the decision and prevents delivery;
/// one received after it cannot withdraw a delivery already permitted, and retires the
/// subscription for the next call instead.
///
/// Generic over the position type only so the ordering rule can be tested without a bus;
/// production passes zbus receive positions.
fn decide<Position: Ord>(decision: Position, period_end: Option<Position>) -> Decision {
    match period_end {
        None => Decision::Deliver,
        Some(end) if end < decision => Decision::Retire,
        Some(_) => Decision::DeliverThenRetire,
    }
}

/// One message read from a subscription's streams.
enum Step {
    /// A signal moved into the backlog.
    Signal,
    /// An ownership notice; `Some` with its receive position when it ends the period.
    Notice(Option<Sequence>),
    /// The ownership stream ended: the connection closed and nothing can be proven any more.
    NoticesClosed,
    /// A signal stream ended.
    SignalsClosed,
}

/// Reads the next message from any of the three streams. Signals join the backlog in receive
/// order. Cancel-safe: a stream's message is taken only in the poll that completes this future.
async fn step(
    live: &mut Receivers,
    owner: &OwnedUniqueName,
    since: Sequence,
    backlog: &mut VecDeque<zbus::Message>,
) -> Step {
    let Receivers {
        events,
        state,
        owner_changes,
    } = live;
    let notice = async {
        match owner_changes.next().await {
            Some(change) => Step::Notice(
                ends_period(&change, owner, since).then(|| change.message().recv_position()),
            ),
            None => Step::NoticesClosed,
        }
    };
    let signal = async {
        let next = futures_lite::future::or(events.next(), async {
            match state {
                Some(state) => state.next().await,
                None => futures_lite::future::pending().await,
            }
        })
        .await;
        match next {
            Some(message) => {
                let position = message.recv_position();
                let at = backlog.partition_point(|queued| queued.recv_position() < position);
                backlog.insert(at, message);
                Step::Signal
            }
            None => Step::SignalsClosed,
        }
    };
    futures_lite::future::or(notice, signal).await
}

/// Why reading during an await had to stop.
enum Interrupt {
    /// The period ended (or ownership can no longer be proven).
    Retired,
    /// The backlog is over its bound.
    Lagged,
    /// A signal stream ended.
    Closed,
}

/// Keeps reading the streams while another await is pending, so that await's reply can be read
/// (QUEUE-1). With `record_end`, a period end does not stop reading: its position is kept in
/// `period_end` for the delivery decision, because a notice that arrives after the decision must
/// not withdraw it. Without it, the first period end interrupts. Never returns otherwise.
async fn keep_reading(
    live: &mut Receivers,
    owner: &OwnedUniqueName,
    since: Sequence,
    backlog: &mut VecDeque<zbus::Message>,
    period_end: &mut Option<Sequence>,
    record_end: bool,
) -> Interrupt {
    loop {
        match step(live, owner, since, backlog).await {
            Step::Signal if backlog.len() > SUBSCRIPTION_BACKLOG => return Interrupt::Lagged,
            Step::Signal | Step::Notice(None) => {}
            Step::Notice(Some(position)) if record_end => {
                period_end.get_or_insert(position);
            }
            Step::Notice(Some(_)) | Step::NoticesClosed => return Interrupt::Retired,
            Step::SignalsClosed => return Interrupt::Closed,
        }
    }
}

/// Reads everything already queued without waiting. zbus hands a message to every stream before
/// it reads the next one, so once a reply is in hand every notice received before it is queued:
/// an empty queue means none exists. Period ends are kept in `period_end`.
async fn settle(
    live: &mut Receivers,
    owner: &OwnedUniqueName,
    since: Sequence,
    backlog: &mut VecDeque<zbus::Message>,
    period_end: &mut Option<Sequence>,
) -> Option<Interrupt> {
    loop {
        match futures_lite::future::poll_once(step(live, owner, since, backlog)).await {
            None => return None,
            Some(Step::Signal) if backlog.len() > SUBSCRIPTION_BACKLOG => {
                return Some(Interrupt::Lagged);
            }
            Some(Step::Signal | Step::Notice(None)) => {}
            Some(Step::Notice(Some(position))) => {
                period_end.get_or_insert(position);
            }
            Some(Step::NoticesClosed) => return Some(Interrupt::Retired),
            Some(Step::SignalsClosed) => return Some(Interrupt::Closed),
        }
    }
}

impl EventSubscription {
    /// Ends the subscription for good and releases its streams and backlog (QUEUE-4).
    fn end(&mut self, why: Ended) -> ClientError {
        self.ended = Some(why);
        self.live = None;
        self.backlog = VecDeque::new();
        why.error()
    }

    fn interrupted(&mut self, interrupt: Interrupt) -> ClientError {
        // A period end already read wins: the loss is the more precise report.
        let interrupt = if self.period_end.is_some() {
            Interrupt::Retired
        } else {
            interrupt
        };
        match interrupt {
            Interrupt::Retired => self.end(Ended::OwnerLost),
            Interrupt::Lagged => self.end(Ended::Lagged),
            Interrupt::Closed => {
                self.live = None;
                self.backlog = VecDeque::new();
                ClientError::EventStreamClosed
            }
        }
    }

    /// The next typed event. On a [`BlueRouteClient::changes`] subscription, invalidations are
    /// skipped; use [`Self::next_change`] there.
    pub async fn next_event(&mut self) -> Result<Event, ClientError> {
        loop {
            if let Change::Event(event) = self.next_change().await? {
                return Ok(event);
            }
        }
    }

    /// The next event or invalidation. Ends with [`ClientError::OwnerLost`] when the bound owner
    /// goes away, and every item is accepted only after that same owner's version is rechecked.
    ///
    /// Delivery decision (FU-02, LC-OWNER, EPOCH-1..3, QUEUE-3): the period is watched the whole
    /// time, including while an item's version is checked, so a held reply cannot delay the loss.
    /// After the check, every notice already queued is settled (each was received before the
    /// final check can be answered), then a final `GetNameOwner` is the decision point: the item
    /// is delivered only if the bound owner holds the name there *and* no notice received before
    /// that reply ended the period. Comparing names alone would let an owner that gave the name
    /// away and took it back (A→B→A) deliver an item of its old period. A period end received
    /// after the reply does not withdraw the delivery; it retires the subscription for the next
    /// call. A retirement seen at any point ends the subscription for good, and a pending check
    /// (and its late reply) is dropped. A failed check is reported as itself unless a period end
    /// is already queued, which wins.
    ///
    /// Cancelling the returned future loses nothing: the item being checked stays first in the
    /// backlog for the next call, and a period end already read stays recorded.
    pub async fn next_change(&mut self) -> Result<Change, ClientError> {
        if let Some(why) = self.ended {
            return Err(why.error());
        }
        if self.period_end.is_some() {
            return Err(self.end(Ended::OwnerLost));
        }
        let Some(live) = self.live.as_mut() else {
            return Err(ClientError::EventStreamClosed);
        };
        let (connection, owner, since) = (&self.connection, &self.owner, self.since);
        let (backlog, period_end) = (&mut self.backlog, &mut self.period_end);

        // Select the next item: the oldest one already received, or the next to arrive.
        while backlog.is_empty() {
            match step(live, owner, since, backlog).await {
                Step::Signal | Step::Notice(None) => {}
                Step::Notice(Some(_)) | Step::NoticesClosed => {
                    return Err(self.end(Ended::OwnerLost));
                }
                Step::SignalsClosed => return Err(self.interrupted(Interrupt::Closed)),
            }
        }

        // Settle what is already queued first, so a period end already received costs no
        // round trip to an owner that has left.
        if let Some(interrupt) = settle(live, owner, since, backlog, period_end).await {
            return Err(self.interrupted(interrupt));
        }
        if period_end.is_some() {
            return Err(self.end(Ended::OwnerLost));
        }

        // The streams only carry this owner's signals; recheck that same owner's version, while
        // still reading, so nothing is accepted on another owner's check, a held reply cannot
        // delay the loss, and the reply itself can be read.
        let checked = futures_lite::future::or(
            async { Ok(negotiate_version_at(connection, owner).await) },
            async { Err(keep_reading(live, owner, since, backlog, period_end, false).await) },
        )
        .await;
        let checked = match checked {
            Err(interrupt) => return Err(self.interrupted(interrupt)),
            Ok(checked) => checked,
        };
        // Everything queued now was received before the final check can be answered.
        if let Some(interrupt) = settle(live, owner, since, backlog, period_end).await {
            return Err(self.interrupted(interrupt));
        }
        if period_end.is_some() {
            // A loss already queued wins over the check's own result, failed or not.
            return Err(self.end(Ended::OwnerLost));
        }
        checked?;

        // The decision point: the checked owner must still be the current one, in the same
        // period. Reading continues meanwhile, recording any period end for the decision.
        let verified =
            futures_lite::future::or(async { Ok(current_owner(connection).await) }, async {
                Err(keep_reading(live, owner, since, backlog, period_end, true).await)
            })
            .await;
        let verified = match verified {
            Err(interrupt) => return Err(self.interrupted(interrupt)),
            Ok(verified) => verified,
        };
        if let Some(interrupt) = settle(live, owner, since, backlog, period_end).await {
            return Err(self.interrupted(interrupt));
        }
        let decision = match verified {
            Ok((current, decision)) if current == *owner => decision,
            Ok(_) => return Err(self.end(Ended::OwnerLost)),
            Err(error) if shows_no_owner(&error) || period_end.is_some() => {
                return Err(self.end(Ended::OwnerLost));
            }
            Err(error) => return Err(error),
        };
        let decided = decide(decision, *period_end);
        if decided == Decision::Retire {
            return Err(self.end(Ended::OwnerLost));
        }
        let message = backlog
            .pop_front()
            .expect("the item being checked is still first in the backlog");
        if decided == Decision::DeliverThenRetire {
            // The permitted delivery stands, but the period is over: end now, releasing the
            // streams, so every later call reports OwnerLost and nothing is held back meanwhile.
            self.end(Ended::OwnerLost);
        }
        if message.header().member().map(|member| member.as_str()) == Some(STATE_CHANGED_SIGNAL) {
            let revision: u64 = message
                .body()
                .deserialize()
                .map_err(|error| ClientError::InvalidTransportPayload(error.to_string()))?;
            Ok(Change::State { revision })
        } else {
            let payload: String = message
                .body()
                .deserialize()
                .map_err(|error| ClientError::InvalidTransportPayload(error.to_string()))?;
            decode_event(&payload)
                .map(Change::Event)
                .map_err(ClientError::Protocol)
        }
    }
}

/// Runs `attempt` (one owner lookup and Version negotiation) until it decides, retrying only after
/// a bus failure, all within one `timeout` budget (RQR-01, DEADLINE-1..6).
///
/// The deadline is taken on the first poll. Every attempt races it, and every retry delay is cut
/// to the time left. After each attempt the deadline is checked before the attempt's outcome is
/// looked at, so an expired budget wins over a reply that became ready with it, compatible or
/// not; branch polling order never decides the result. A budget too large to represent as an
/// instant never expires.
async fn negotiate_within<Attempt, Negotiation>(
    timeout: Duration,
    mut attempt: Attempt,
) -> Result<ApiVersion, ClientError>
where
    Attempt: FnMut() -> Negotiation,
    Negotiation: Future<Output = Result<ApiVersion, ClientError>>,
{
    let deadline = Instant::now().checked_add(timeout);
    let expired = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
    let timed_out = |last_error: String| ClientError::ReconnectTimeout {
        timeout,
        last_error,
    };
    // The most recent bus failure, kept as context for a timeout.
    let mut failure: Option<String> = None;
    loop {
        if expired() {
            // Before the first attempt (a zero budget) or after a capped retry delay.
            return Err(timed_out(failure.unwrap_or_else(|| {
                "the budget ran out before the daemon was contacted".to_owned()
            })));
        }
        let outcome = match deadline {
            Some(deadline) => {
                futures_lite::future::or(async { Some(attempt().await) }, async {
                    Timer::at(deadline).await;
                    None
                })
                .await
            }
            None => Some(attempt().await),
        };
        if expired() {
            let what = match &outcome {
                None => {
                    "the owner lookup or Version negotiation did not answer before the deadline"
                        .to_owned()
                }
                Some(Ok(_)) => "the daemon answered only after the deadline".to_owned(),
                Some(Err(ClientError::Bus(error))) => {
                    format!("the last attempt failed only after the deadline: {error}")
                }
                Some(Err(error)) => {
                    format!("the last attempt ended only after the deadline: {error}")
                }
            };
            return Err(timed_out(match failure {
                Some(failure) => format!("{what}; the previous attempt failed: {failure}"),
                None => what,
            }));
        }
        match outcome {
            Some(Ok(version)) => return Ok(version),
            Some(Err(ClientError::Bus(error))) => {
                failure = Some(error.to_string());
                let retry = Instant::now() + RECONNECT_POLL_INTERVAL;
                Timer::at(deadline.map_or(retry, |deadline| deadline.min(retry))).await;
            }
            // Incompatibility and every other typed failure decide at once.
            Some(Err(error)) => return Err(error),
            // The deadline's timer fired, so the check above has already returned.
            None => {
                return Err(timed_out(
                    "the owner lookup or Version negotiation did not answer before the deadline"
                        .to_owned(),
                ));
            }
        }
    }
}

/// Resolve the well-known name to its current unique owner and check that owner's version there.
async fn validated_owner(connection: &Connection) -> Result<ValidatedOwner, ClientError> {
    let (name, since) = current_owner(connection).await?;
    let version = negotiate_version_at(connection, &name).await?;
    Ok(ValidatedOwner {
        name,
        version,
        since,
    })
}

/// The unique bus name currently owning the daemon's well-known name, and the receive position
/// of the reply that said so. No owner is `NameHasNoOwner`, which classifies as unavailable.
async fn current_owner(
    connection: &Connection,
) -> Result<(OwnedUniqueName, Sequence), ClientError> {
    let bus_error = |error: zbus::Error| {
        ClientError::Bus(zbus::Error::FDO(Box::new(zbus::fdo::Error::from(error))))
    };
    let reply = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &DBUS_SERVICE_NAME,
        )
        .await
        .map_err(bus_error)?;
    let name: OwnedUniqueName = reply.body().deserialize().map_err(bus_error)?;
    Ok((name, reply.recv_position()))
}

async fn negotiate_version_at(
    connection: &Connection,
    owner: &OwnedUniqueName,
) -> Result<ApiVersion, ClientError> {
    let proxy = owner_proxy(connection, owner).await?;
    let (major, minor): (u16, u16) = proxy.call("Version", &()).await.map_err(ClientError::Bus)?;
    let server = ApiVersion::new(major, minor);
    if !ApiVersion::CURRENT.is_compatible_with_server(server) {
        return Err(ClientError::IncompatibleVersion {
            client: ApiVersion::CURRENT,
            server,
        });
    }
    Ok(server)
}

/// A proxy addressed to one unique owner, never to the well-known name.
async fn owner_proxy(
    connection: &Connection,
    owner: &OwnedUniqueName,
) -> Result<Proxy<'static>, ClientError> {
    Proxy::new_owned(
        connection.clone(),
        owner.to_string(),
        DBUS_OBJECT_PATH,
        DBUS_INTERFACE_NAME,
    )
    .await
    .map_err(ClientError::Bus)
}

fn response_kind(response: &Response) -> &'static str {
    match response {
        Response::Ack => "ack",
        Response::Status(_) => "status",
        Response::Capabilities(_) => "capabilities",
        Response::Networks(_) => "networks",
        Response::Nodes(_) => "nodes",
        Response::Node(_) => "node",
        Response::Diagnostics(_) => "diagnostics",
        Response::ClientLink(_) => "client_link",
    }
}

/// Stable error categories shared by the CLI and other daemon clients.
/// Only exact D-Bus protocol error names classify remote failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientErrorKind {
    Unavailable,
    Authorization,
    Unsupported,
    Operational,
    /// The network's private IPv4 range overlaps one already in use on this computer
    /// (`blueroute_protocol::ADDRESS_CONFLICT_ERROR`, API 1.6).
    AddressConflict,
}

fn remote_error_kind(name: &str) -> ClientErrorKind {
    match name {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            ClientErrorKind::Authorization
        }
        "org.freedesktop.DBus.Error.NotSupported" => ClientErrorKind::Unsupported,
        blueroute_protocol::ADDRESS_CONFLICT_ERROR => ClientErrorKind::AddressConflict,
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.NoReply"
        | "org.freedesktop.DBus.Error.NoServer" => ClientErrorKind::Unavailable,
        _ => ClientErrorKind::Operational,
    }
}

#[derive(Debug)]
pub enum ClientError {
    Bus(zbus::Error),
    Protocol(ProtocolCodecError),
    IncompatibleVersion {
        client: ApiVersion,
        server: ApiVersion,
    },
    UnexpectedResponse {
        expected: &'static str,
        actual: &'static str,
    },
    InvalidTransportPayload(String),
    EventStreamClosed,
    /// The daemon owner this subscription was bound to gave up its name or disappeared.
    OwnerLost,
    /// The subscription fell more than `limit` items behind the daemon and ended rather than
    /// stall its connection; reread state and subscribe again (QR-QUEUE).
    SubscriptionLagged {
        limit: usize,
    },
    ReconnectTimeout {
        timeout: Duration,
        last_error: String,
    },
}

impl ClientError {
    /// Classify a specific typed client failure; do not interpret
    /// peer-controlled textual diagnostics as authorization decisions.
    pub fn kind(&self) -> ClientErrorKind {
        match self {
            Self::IncompatibleVersion { .. } | Self::ReconnectTimeout { .. } | Self::OwnerLost => {
                ClientErrorKind::Unavailable
            }
            Self::Bus(zbus::Error::MethodError(name, _, _)) => remote_error_kind(name.as_str()),
            Self::Bus(zbus::Error::FDO(error)) => match error.as_ref() {
                zbus::fdo::Error::AccessDenied(_)
                | zbus::fdo::Error::AuthFailed(_)
                | zbus::fdo::Error::InteractiveAuthorizationRequired(_) => {
                    ClientErrorKind::Authorization
                }
                zbus::fdo::Error::NotSupported(_) => ClientErrorKind::Unsupported,
                zbus::fdo::Error::ServiceUnknown(_)
                | zbus::fdo::Error::NameHasNoOwner(_)
                | zbus::fdo::Error::NoReply(_)
                | zbus::fdo::Error::NoServer(_) => ClientErrorKind::Unavailable,
                _ => ClientErrorKind::Operational,
            },
            Self::Bus(
                zbus::Error::InputOutput(_) | zbus::Error::Address(_) | zbus::Error::Handshake(_),
            ) => ClientErrorKind::Unavailable,
            _ => ClientErrorKind::Operational,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(formatter, "D-Bus operation failed: {error}"),
            Self::Protocol(error) => write!(formatter, "protocol payload failed: {error}"),
            Self::IncompatibleVersion { client, server } => write!(
                formatter,
                "incompatible BlueRoute API: client={client} server={server}"
            ),
            Self::UnexpectedResponse { expected, actual } => write!(
                formatter,
                "daemon returned response type {actual} where {expected} was required"
            ),
            Self::InvalidTransportPayload(error) => {
                write!(
                    formatter,
                    "daemon event carried an invalid D-Bus payload: {error}"
                )
            }
            Self::EventStreamClosed => formatter.write_str("daemon event stream closed"),
            Self::OwnerLost => formatter.write_str("the daemon went away or was replaced"),
            Self::SubscriptionLagged { limit } => write!(
                formatter,
                "the event subscription fell more than {limit} items behind and ended"
            ),
            Self::ReconnectTimeout {
                timeout,
                last_error,
            } => write!(
                formatter,
                "compatible daemon did not return within {timeout:?}: {last_error}"
            ),
        }
    }
}

impl std::error::Error for ClientError {}

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

#[cfg(test)]
mod error_class_tests {
    use super::*;

    // QR-QUEUE-07 / QUEUE-3: the receive-order rule at the decision point, which a real broker
    // cannot place deterministically on either side of the final reply. The real-broker tests in
    // `tests/queue_saturation.rs` and `tests/ownership_period.rs` cover the pre-decision side and
    // the transport; this is the proof for the post-decision side.
    #[test]
    fn qr_queue_a_period_end_before_the_decision_withholds_and_one_after_retires_next() {
        let decision = 10_u64;
        assert_eq!(decide(decision, None), Decision::Deliver);
        assert_eq!(decide(decision, Some(9)), Decision::Retire);
        assert_eq!(decide(decision, Some(1)), Decision::Retire);
        // Received after the final reply: the permitted delivery stands, the next call retires.
        assert_eq!(decide(decision, Some(11)), Decision::DeliverThenRetire);
        assert_eq!(
            decide(decision, Some(u64::MAX)),
            Decision::DeliverThenRetire
        );
        // Positions on one connection are unique, so equality cannot occur; were it ever to,
        // the notice is not before the decision and the delivery is kept.
        assert_eq!(decide(decision, Some(10)), Decision::DeliverThenRetire);
    }

    #[test]
    fn qr_queue_an_overflow_is_typed_operational_and_names_its_limit() {
        let lagged = Ended::Lagged.error();
        assert!(matches!(
            lagged,
            ClientError::SubscriptionLagged { limit } if limit == SUBSCRIPTION_BACKLOG
        ));
        assert_eq!(lagged.kind(), ClientErrorKind::Operational);
        assert!(lagged.to_string().contains("256"));
        assert!(matches!(Ended::OwnerLost.error(), ClientError::OwnerLost));
    }

    #[test]
    fn lc_owner_only_a_missing_owner_retires_on_a_failed_final_check() {
        let fdo = |error| ClientError::Bus(zbus::Error::FDO(Box::new(error)));
        assert!(shows_no_owner(&fdo(zbus::fdo::Error::NameHasNoOwner(
            "gone".into()
        ))));
        for transient in [
            fdo(zbus::fdo::Error::Timeout("slow".into())),
            fdo(zbus::fdo::Error::NoReply("lost".into())),
            fdo(zbus::fdo::Error::LimitsExceeded("busy".into())),
            ClientError::Bus(zbus::Error::InputOutput(std::sync::Arc::new(
                std::io::Error::other("interrupted"),
            ))),
        ] {
            assert!(!shows_no_owner(&transient), "{transient:?}");
        }
    }

    #[test]
    fn error_names_are_exact_and_never_classify_untrusted_message_text() {
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.AccessDenied"),
            ClientErrorKind::Authorization
        );
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.NotSupported"),
            ClientErrorKind::Unsupported
        );
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.ServiceUnknown"),
            ClientErrorKind::Unavailable
        );
        assert_eq!(
            remote_error_kind("org.blueroute.Error.AddressConflict"),
            ClientErrorKind::AddressConflict
        );
        for suspicious in [
            "AccessDenied",
            "org.freedesktop.DBus.Error.AccessDenied.extra",
            "Error: AccessDenied",
            "org.freedesktop.DBus.Error.Failed",
            "AddressConflict",
            "org.blueroute.Error.AddressConflict.extra",
            "org.blueroute.Error.Other",
        ] {
            assert_eq!(remote_error_kind(suspicious), ClientErrorKind::Operational);
        }
    }

    #[test]
    fn version_mismatch_is_unavailable_and_unrelated_errors_remain_operational() {
        assert_eq!(
            ClientError::IncompatibleVersion {
                client: ApiVersion::new(1, 0),
                server: ApiVersion::new(2, 0),
            }
            .kind(),
            ClientErrorKind::Unavailable
        );
        assert_eq!(
            ClientError::EventStreamClosed.kind(),
            ClientErrorKind::Operational
        );
        assert_eq!(
            ClientError::Bus(zbus::Error::FDO(Box::new(zbus::fdo::Error::AccessDenied(
                "denied".into()
            ))))
            .kind(),
            ClientErrorKind::Authorization
        );
    }
}

/// RC-TEST-02/03/04: the whole-operation budget through the internal seam that `reconnect` uses,
/// with scripted attempts. A real broker answers `GetNameOwner` at once, so a delayed owner lookup
/// is scripted here; the real-broker held-Version regressions are in `tests/reconnect_deadline.rs`.
#[cfg(test)]
mod deadline_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    const NEWER: ApiVersion =
        ApiVersion::new(ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor + 1);
    const INCOMPATIBLE: ApiVersion = ApiVersion::new(ApiVersion::CURRENT.major + 1, 0);
    /// Scheduler tolerance on top of a budget.
    const TOLERANCE: Duration = Duration::from_millis(150);

    fn absent() -> ClientError {
        ClientError::Bus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::NameHasNoOwner("no owner".into()),
        )))
    }

    fn checked(server: ApiVersion) -> Result<ApiVersion, ClientError> {
        if ApiVersion::CURRENT.is_compatible_with_server(server) {
            Ok(server)
        } else {
            Err(ClientError::IncompatibleVersion {
                client: ApiVersion::CURRENT,
                server,
            })
        }
    }

    fn timeout_of(outcome: Result<ApiVersion, ClientError>, budget: Duration) -> String {
        match outcome {
            Err(error @ ClientError::ReconnectTimeout { timeout, .. }) if timeout == budget => {
                assert_eq!(error.kind(), ClientErrorKind::Unavailable);
                let ClientError::ReconnectTimeout { last_error, .. } = error else {
                    unreachable!()
                };
                last_error
            }
            other => panic!("expected ReconnectTimeout({budget:?}): {other:?}"),
        }
    }

    /// RC-TEST-02: a lookup that never answers is bounded too, not only the Version stage, and
    /// the Version stage is never reached.
    #[test]
    fn rc_a_delayed_owner_lookup_is_bounded_by_the_whole_budget() {
        let budget = Duration::from_millis(50);
        let lookups = Rc::new(Cell::new(0));
        let versions = Rc::new(Cell::new(0));
        let started = Instant::now();
        let outcome = futures_lite::future::block_on(negotiate_within(budget, || {
            let (lookups, versions) = (lookups.clone(), versions.clone());
            async move {
                lookups.set(lookups.get() + 1);
                // The owner lookup is held well past the budget.
                Timer::after(Duration::from_secs(10)).await;
                versions.set(versions.get() + 1);
                checked(ApiVersion::CURRENT)
            }
        }));
        let elapsed = started.elapsed();
        let last_error = timeout_of(outcome, budget);
        assert!(elapsed < budget + TOLERANCE, "took {elapsed:?}");
        assert!(last_error.contains("did not answer"), "{last_error}");
        assert!(
            !last_error.contains("D-Bus operation failed"),
            "{last_error}"
        );
        assert_eq!((lookups.get(), versions.get()), (1, 0));
    }

    /// RC-TEST-03 / DEADLINE-6: a zero budget makes no attempt.
    #[test]
    fn rc_a_zero_budget_makes_no_attempt() {
        let attempts = Cell::new(0);
        let outcome = futures_lite::future::block_on(negotiate_within(Duration::ZERO, || {
            attempts.set(attempts.get() + 1);
            async { checked(ApiVersion::CURRENT) }
        }));
        let last_error = timeout_of(outcome, Duration::ZERO);
        assert!(
            last_error.contains("before the daemon was contacted"),
            "{last_error}"
        );
        assert_eq!(attempts.get(), 0);
    }

    /// RC-TEST-03 / DEADLINE-3: retry delays are cut to the time left and nothing is attempted
    /// after expiry; the last bus failure is kept as context.
    #[test]
    fn rc_retry_delays_are_capped_and_stop_at_the_deadline() {
        // (budget, most attempts, slack): the short budget must stay below one 100 ms interval.
        for (budget, most, slack) in [(20, 1, 75), (250, 3, 150)] {
            let budget = Duration::from_millis(budget);
            let started = Instant::now();
            let late = Cell::new(0);
            let attempts = Cell::new(0);
            let outcome = futures_lite::future::block_on(negotiate_within(budget, || {
                attempts.set(attempts.get() + 1);
                if started.elapsed() >= budget {
                    late.set(late.get() + 1);
                }
                async { Err(absent()) }
            }));
            let elapsed = started.elapsed();
            let last_error = timeout_of(outcome, budget);
            assert!(last_error.contains("NameHasNoOwner"), "{last_error}");
            assert!(elapsed >= budget, "gave up early: {elapsed:?}");
            assert!(
                elapsed < budget + Duration::from_millis(slack),
                "{budget:?} took {elapsed:?}"
            );
            assert_eq!(late.get(), 0, "attempted after the deadline");
            assert!(
                (1..=most).contains(&attempts.get()),
                "{budget:?}: {} attempts",
                attempts.get()
            );
        }
    }

    /// RC-TEST-04 / DEADLINE-4: an attempt that decides only after the deadline (the executor
    /// resumed late) loses to it, compatible or not.
    #[test]
    fn rc_an_outcome_decided_after_the_deadline_loses_to_it() {
        let budget = Duration::from_millis(20);
        for server in [NEWER, INCOMPATIBLE] {
            let outcome = futures_lite::future::block_on(negotiate_within(budget, || async move {
                // Blocks the executor past the deadline, so the reply and the timer are both
                // ready when the operation is next looked at.
                std::thread::sleep(budget * 3);
                checked(server)
            }));
            let last_error = timeout_of(outcome, budget);
            assert!(
                last_error.contains("only after the deadline"),
                "{server}: {last_error}"
            );
        }
    }

    /// DEADLINE-4: timely outcomes stand; incompatibility is not retried.
    #[test]
    fn rc_timely_outcomes_decide_at_once() {
        let budget = Duration::from_secs(5);
        let attempts = Cell::new(0);
        let outcome = futures_lite::future::block_on(negotiate_within(budget, || {
            attempts.set(attempts.get() + 1);
            async { checked(INCOMPATIBLE) }
        }));
        assert!(
            matches!(outcome, Err(ClientError::IncompatibleVersion { server, .. })
            if server == INCOMPATIBLE)
        );
        assert_eq!(attempts.get(), 1);

        // A bus failure then a compatible reply: retried once, accepted in time.
        let attempts = Cell::new(0);
        let outcome = futures_lite::future::block_on(negotiate_within(budget, || {
            attempts.set(attempts.get() + 1);
            let first = attempts.get() == 1;
            async move { if first { Err(absent()) } else { checked(NEWER) } }
        }));
        assert!(matches!(outcome, Ok(version) if version == NEWER));
        assert_eq!(attempts.get(), 2);

        // Other typed failures are not retried either.
        let outcome = futures_lite::future::block_on(negotiate_within(budget, || async {
            Err(ClientError::OwnerLost)
        }));
        assert!(matches!(outcome, Err(ClientError::OwnerLost)));
    }

    /// DEADLINE-1: a budget too large to represent as an instant never expires.
    #[test]
    fn rc_an_unrepresentable_budget_does_not_overflow() {
        let outcome = futures_lite::future::block_on(negotiate_within(Duration::MAX, || async {
            checked(NEWER)
        }));
        assert!(matches!(outcome, Ok(version) if version == NEWER));
    }

    /// DEADLINE-5: an attempt that fails only after the deadline keeps its bus diagnostic and
    /// is not described as an answer.
    #[test]
    fn rc_a_failure_after_the_deadline_keeps_its_diagnostic() {
        let budget = Duration::from_millis(20);
        let outcome = futures_lite::future::block_on(negotiate_within(budget, || async move {
            std::thread::sleep(budget * 3);
            Err(absent())
        }));
        let last_error = timeout_of(outcome, budget);
        assert!(
            last_error.contains("failed only after the deadline"),
            "{last_error}"
        );
        assert!(last_error.contains("NameHasNoOwner"), "{last_error}");
        assert!(!last_error.contains("answered"), "{last_error}");
    }

    /// DEADLINE-1: the budget starts when the operation is first polled, not when its future
    /// is created. `tests/reconnect_deadline.rs` checks the same through the public `reconnect`.
    #[test]
    fn rc_the_budget_starts_at_the_first_poll() {
        let budget = Duration::from_millis(50);
        let operation = negotiate_within(budget, || async { checked(NEWER) });
        std::thread::sleep(budget * 2);
        let outcome = futures_lite::future::block_on(operation);
        assert!(
            matches!(outcome, Ok(version) if version == NEWER),
            "{outcome:?}"
        );
    }
}
