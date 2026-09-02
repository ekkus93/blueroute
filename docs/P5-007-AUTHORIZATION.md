# P5-007 — D-Bus and PolicyKit Authorization

P5-007 defines the local privilege boundary between unprivileged BlueRoute front ends and the root-run `blueroute-daemon`.

The architectural invariant is unchanged: **CLI, TUI, and desktop processes are not run as root**. They communicate with the system daemon over the versioned system D-Bus API. The daemon is the only process that owns privileged Linux networking state.

## D-Bus boundary

The system-bus policy separates **service ownership** from **API access**:

- only `root` may own `org.blueroute.Service1`;
- local unprivileged processes may send requests to `org.blueroute.Service1`;
- allowing a process to send a D-Bus method call does **not** authorize a privileged BlueRoute command.

This separation is required so ordinary users can inspect status/capabilities without elevating an entire front end.

## Command classes

The daemon classifies every current typed `Command` before dispatch.

### Read-only

These operations do not contact PolicyKit:

- `GetStatus`
- `GetCapabilities`
- `ListNetworks`
- `ListNodes`
- `GetNode`
- `GetDiagnostics`

The direct D-Bus `Version`, `Status`, and `Capabilities` methods are also read-only.

### Normal mutations

These commands require PolicyKit action `org.blueroute.modify`:

- `CreateNetwork`
- `JoinNetwork`
- `LeaveNetwork`
- `SetDeviceName`
- `StartDiscovery`
- `StopDiscovery`
- `TrustPeer`
- `ForgetPeer`

### Internet sharing

The reserved future `SetInternetSharing` command is assigned a separate action:

- `org.blueroute.internet-sharing`

Internet gateway/NAT functionality remains unimplemented, but reserving a distinct authorization action now prevents a later high-impact gateway operation from silently inheriting a broader networking permission.

The classification match is exhaustive. Adding a future `Command` variant requires an explicit authorization-policy decision at compile time.

## PolicyKit check

For a mutating command, the daemon:

1. successfully decodes the typed command;
2. obtains the original D-Bus message sender from the method-call header;
3. constructs a PolicyKit `system-bus-name` subject using that unique sender name;
4. calls `org.freedesktop.PolicyKit1.Authority.CheckAuthorization`;
5. dispatches the command only when PolicyKit returns `is_authorized=true`.

The daemon passes PolicyKit's `AllowUserInteraction` flag because normal mutation requests originate from an explicit local frontend action. If no suitable authentication agent exists, PolicyKit may deny/challenge rather than authorize.

Malformed protocol payloads fail with `InvalidArgs` before authorization. This avoids prompting for data that cannot become a valid command.

## Fail-closed behavior

Mutating requests fail with D-Bus `AccessDenied` when:

- the incoming method call has no usable sender identity;
- PolicyKit cannot be reached;
- the authorization check itself errors;
- PolicyKit denies the requested action.

There is deliberately **no** fallback that:

- treats all local users as trusted;
- runs a frontend with `sudo`;
- shells out to `pkexec`/`sudo` from the daemon;
- performs the mutation first and reports authorization afterward;
- silently skips authorization when PolicyKit is unavailable.

Authorization is evaluated before mutation dispatch. A currently unimplemented command therefore produces `NotSupported` only after its required authorization succeeds; a denied caller never reaches that handler.

## Shipped PolicyKit defaults

`packaging/polkit/org.blueroute.policy` defines both BlueRoute actions with conservative defaults:

```text
allow_any      = no
allow_inactive = auth_admin
allow_active   = auth_admin_keep
```

An active local user may authenticate as an administrator and retain that authorization according to PolicyKit's normal temporary-authorization semantics. Remote/arbitrary or inactive callers are not granted implicit mutation access.

Distribution administrators may override PolicyKit policy using standard PolicyKit mechanisms without changing BlueRoute code.

## CI acceptance

The daemon's isolated D-Bus test runs against a mock `org.freedesktop.PolicyKit1.Authority` and proves:

- read-only requests do not contact PolicyKit;
- malformed payloads fail before PolicyKit;
- a denied mutation returns `AccessDenied`;
- the PolicyKit subject is the original `system-bus-name` sender;
- normal mutations use `org.blueroute.modify`;
- Internet sharing uses `org.blueroute.internet-sharing`;
- an authorized mutation passes the security gate and reaches the current command handler;
- PolicyKit disappearance causes later mutations to fail closed.

CI also parses the D-Bus and PolicyKit XML and asserts that the bus policy contains only the intended owner/send grants and that both PolicyKit actions retain the conservative defaults above.

## Live Debian acceptance

Before P5-007 is marked complete on the initial Debian baseline, hardware/system acceptance should prove:

1. the normal login user can call `Version`, `Status`, and `Capabilities` without `sudo` or a PolicyKit prompt;
2. an unprivileged caller that cannot authenticate cannot cause a mutating command to pass the authorization boundary;
3. the daemon remains root-owned while the client remains unprivileged;
4. denial is visible as a typed D-Bus authorization error rather than a silent no-op or privileged fallback.

P5-007 does not require an implemented `CreateNetwork` data-plane mutation; an authorized command may still return `NotSupported` until P6 implements the operation. The acceptance criterion is that authorization is enforced **before** that future mutation point.
