from pathlib import Path

pan = Path('crates/blueroute-linux/src/pan.rs')
text = pan.read_text()

replacements = [
    (
        'use std::collections::HashMap;\n\nuse futures_lite::StreamExt;\n',
        'use std::collections::HashMap;\nuse std::time::Duration;\n\nuse async_io::Timer;\nuse futures_lite::{future, StreamExt};\n',
    ),
    (
        'const SIGNAL_QUEUE_CAPACITY: usize = 64;\n',
        'const SIGNAL_QUEUE_CAPACITY: usize = 64;\nconst PANU_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);\n',
    ),
    (
'''async fn connect_panu(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<PanAttachment, CoreError> {
    let proxy = network_proxy(connection, peer).await?;
    match proxy.call_method(CONNECT_METHOD, &(REMOTE_NAP_ROLE,)).await {
        Ok(reply) => {
            let interface: String = reply.body().deserialize().map_err(|error| {
                pan_error(
                    ErrorKind::ProtocolError,
                    "BlueZ returned an invalid PAN interface name",
                    error,
                )
            })?;
            panu_attachment(peer, interface)
        }
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.bluez.Error.AlreadyConnected" =>
        {
            current_panu_attachment(connection, peer)
                .await?
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorKind::ProtocolError,
                        "BlueZ reported an existing PAN connection without an active interface",
                    )
                })
        }
        Err(error) => Err(connect_error(error)),
    }
}
''',
'''async fn connect_panu(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<PanAttachment, CoreError> {
    if let Some(attachment) = current_panu_attachment(connection, peer).await? {
        return Ok(attachment);
    }

    let proxy = network_proxy(connection, peer).await?;
    let connect = async {
        proxy
            .call_method(CONNECT_METHOD, &(REMOTE_NAP_ROLE,))
            .await
            .map(Some)
    };
    let timeout = async {
        Timer::after(PANU_CONNECT_TIMEOUT).await;
        Ok::<Option<Message>, zbus::Error>(None)
    };

    match future::race(connect, timeout).await {
        Ok(Some(reply)) => {
            let interface: String = reply.body().deserialize().map_err(|error| {
                pan_error(
                    ErrorKind::ProtocolError,
                    "BlueZ returned an invalid PAN interface name",
                    error,
                )
            })?;
            panu_attachment(peer, interface)
        }
        Ok(None) => match abort_pending_panu_connect(connection, peer).await {
            Ok(()) => Err(CoreError::new(
                ErrorKind::PanFailure,
                "Bluetooth PAN connection timed out and was cancelled",
            )),
            Err(cleanup) => Err(CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "Bluetooth PAN connection timed out and cleanup failed",
                cleanup
                    .diagnostic()
                    .unwrap_or_else(|| cleanup.message())
                    .to_owned(),
            )),
        },
        Err(error) => {
            if let Ok(Some(attachment)) = current_panu_attachment(connection, peer).await {
                return Ok(attachment);
            }
            if connect_is_in_progress(&error) {
                return Err(pan_error(
                    ErrorKind::InvalidState,
                    "another Bluetooth PAN connection attempt is already in progress",
                    error,
                ));
            }
            if connect_is_already_connected(&error) {
                return Err(CoreError::new(
                    ErrorKind::ProtocolError,
                    "BlueZ reported an existing PAN connection without an active interface",
                ));
            }
            Err(connect_error(error))
        }
    }
}

async fn abort_pending_panu_connect(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<(), CoreError> {
    let proxy = network_proxy(connection, peer).await?;
    match proxy.call_method(DISCONNECT_METHOD, &()).await {
        Ok(_) => Ok(()),
        Err(error) if disconnect_is_already_absent(&error) => Ok(()),
        Err(error) => Err(disconnect_error(error)),
    }
}
''',
    ),
    (
'''fn connect_error(error: zbus::Error) -> CoreError {
''',
'''fn connect_is_already_connected(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.bluez.Error.AlreadyConnected"
    )
}

fn connect_is_in_progress(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, detail, _) => {
            name.as_str() == "org.bluez.Error.InProgress"
                || (name.as_str() == "org.bluez.Error.Failed"
                    && detail
                        .as_deref()
                        .is_some_and(|message| message.contains("already in progress")))
        }
        _ => false,
    }
}

fn connect_error(error: zbus::Error) -> CoreError {
''',
    ),
]

for old, new in replacements:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'expected exactly one match, found {count}: {old[:80]!r}')
    text = text.replace(old, new, 1)

pan.write_text(text)

doc = Path('docs/P4-005-PANU.md')
doc_text = doc.read_text()
old = 'If BlueZ reports `AlreadyConnected`, BlueRoute reconciles the authoritative `Network1.Connected` and `Network1.Interface` properties and returns the existing attachment rather than treating an already-satisfied connection as a fatal error.\n\nCommon BlueZ errors are mapped into typed `CoreError` categories.'
new = 'Before starting a new connection, BlueRoute reconciles the authoritative `Network1.Connected` and `Network1.Interface` properties and returns an existing attachment when the requested state is already satisfied. The `Connect("nap")` call is bounded to 30 seconds. If it times out, BlueRoute calls `Network1.Disconnect()` to abort the pending attempt, as permitted by the BlueZ Network API, so a stalled operation is not left behind for the next retry. BlueZ `InProgress` responses (including the older generic `Failed: Operation already in progress` form observed on hardware) are surfaced as `InvalidState` rather than an undifferentiated PAN failure.\n\nCommon BlueZ errors are mapped into typed `CoreError` categories.'
if doc_text.count(old) != 1:
    raise SystemExit('P4-005 documentation anchor not found exactly once')
doc.write_text(doc_text.replace(old, new, 1))

todo = Path('docs/TODO.md')
todo_text = todo.read_text()
old = '| P4-005 | `[-]` | BlueZ Network1 PANU connect/interface mapping, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending. |'
new = '| P4-005 | `[-]` | BlueZ Network1 PANU connect/interface mapping, bounded connect timeout/cancellation, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending. |'
if todo_text.count(old) != 1:
    raise SystemExit('P4-005 TODO summary anchor not found exactly once')
todo.write_text(todo_text.replace(old, new, 1))
