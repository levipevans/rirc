# Security review of rirc 0.1.0

**November 24, 2026 · Windows x64 · `we-testin-opencode`**

Four high-priority findings affect passwords or message recipients. A normal keyed join can turn the key into a channel name. A channel conversation can be labeled private. Distinct people or channels can share a conversation buffer on some networks. Registration can also transmit reusable credentials over plaintext. The first two behaviors were observed using the actual command handlers and an in-memory outgoing queue. The other two were traced through the source.

**Immediate precautions.** Use another client for keyed joins and sensitive conversations until recipient handling is fixed. For authenticated connections, start a fresh process with an explicit TLS configuration. If a real key was supplied through the affected `/join` path, rotate that channel key. If a real account or server password was used on a plaintext connection, rotate that credential over a trusted connection. The review used synthetic data and found no evidence about prior disclosure.

## Scope and test results

The review covered the 16 Rust source files, the manifest, lockfile, and README. The reviewed source, manifest, lockfile, and README match commit `febfaf942b31c2bf70178abf68ccf118723d16dc` after line-ending normalization. The other agent initialized the repository during the review. Of the 21 files in the original snapshot, only `.gitignore` differed from that commit. Builds and test fixtures used a separate copy and target directory. The Markdown report is the only file this review added to the project.

| Check | Result and interpretation |
|---|---|
| Daybreak Defense Factory | The access check returned `access: denied` and `isEligibleWorkspace: false`. No cloud scan ran and there is no Daybreak run ID. The findings below come from local review and independent advisory matching. |
| Existing tests, `cargo test --offline --locked` | **25 passed, 2 failed, 2 ignored.** Both failures required a local TCP connection (`speaks_to_a_local_server` and `list_round_trips_over_a_socket`). A separate minimal loopback check received Windows error 10013, access forbidden. Those integration checks remain unverified. The two ignored tests contact Libera.Chat and were left disabled. |
| Five independent, offline conformance checks | Default TLS on port 6697, an ordinary direct message, and an ordinary single-channel join passed. A standard keyed join and channel recipient classification failed. The checks captured outbound commands in memory. No real credential, IRC service, or clipboard was used. |
| `cargo clippy --offline --locked --all-targets` | Completed with four distinct warnings about match structure, method naming, and enum size. No automatic fixes were applied. |
| Dependency review | All **122** third-party package/version entries in `Cargo.lock`, including target-specific entries, were checked against a pinned RustSec database. Four advisory records matched. Details appear below. |
| Coverage limits | Windows source build with Rust 1.99.0. Live server behavior, TLS handshakes, macOS/Linux execution, host TLS library patch levels, and provenance of the prebuilt `dist/rirc.exe` were outside the verified results. |

Severity reflects the likely impact on a desktop IRC user and the conditions required. **High** marks plausible loss of a secret or disclosure to the wrong audience. **Medium** covers availability, authentication expectations, identity displays, and conditional local exposure. **Low** marks a defense gap with narrower prerequisites. Source locations refer to the revision above.

## Findings at a glance

| ID | Severity | Finding | Evidence |
|---|---|---|---|
| 01 | High | Password authentication can use plaintext, including when a TLS request reuses a plaintext session | Source trace and existing port-selection tests |
| 02 | High | A keyed join turns the key into another channel name | Observed in memory |
| 03 | High | Group destinations appear as private conversations | Channel case observed in memory, status-target path traced |
| 04 | High | Server-specific name rules can merge distinct recipients | Source and protocol specification |
| 05 | Medium | Network input can exhaust memory or starve the interface | Source and complexity analysis |
| 06 | Medium | Malformed server capability metadata can terminate the client | Unchecked Rust slice in the UI thread |
| 07 | Medium | Partial frames can lose bytes, and legacy text can disconnect a session | Source and Tokio API contract |
| 08 | Medium | SASL credentials and autojoin are insufficiently tied to authentication state | Source and IRCv3 specification |
| 09 | Medium | Account, operator, and secure-connection labels can identify the wrong person | Source trace |
| 10 | Medium | Automatic version replies can form a traffic loop | Source and IRC notice rules |
| 11 | Medium | Secrets reach command history, transcripts, or clipboard helper processes | Source and process API contract |
| 12 | Low | Outbound IRC commands lack centralized delimiter and byte-length checks | Source trace |

### 01 · High · Credentials can leave over plaintext

**Evidence and impact.** Port 6667 implicitly selects plaintext unless TLS is explicitly forced. The CLI accepts a server password and SASL credentials in the same configuration. The connection task returns a plain TCP stream when configured that way. Registration immediately sends the server password, and the SASL handler can send the reversibly encoded PLAIN credentials. Someone able to observe that connection can recover whichever credential was transmitted. TLS connections themselves use the native connector with the hostname, and a handshake error returns an error. See `src/config.rs:148–170,194–211,247–259`, `src/irc.rs:239–254`, `src/app.rs:685–695,770–783,811–819`. [RFC 4616, security considerations](https://datatracker.ietf.org/doc/html/rfc4616#section-5)

**Reconnect also matters.** An interactive connection lookup uses host and port alone. When the user explicitly requests TLS for an existing plaintext endpoint, the app selects the existing connection or reconnects with its stored transport. The explicit security choice is lost. See `src/app.rs:2567–2621,2671–2685,248–255`.

**Mitigation and acceptance.** Require verified TLS before queuing either automatic credential, regardless of port. Make plaintext an explicit choice, treat transport as part of the connection identity, and reject or visibly resolve conflicting session settings. Verify that an explicit TLS request establishes TLS or fails, and that every plaintext registration path emits zero credential-bearing frames.

### 02 · High · A keyed join exposes the key as a channel name

**Evidence and impact.** Standard IRC accepts a channel list followed by a separate, optional key list. The `/join` handler splits on both commas and whitespace, adds a channel prefix to each token, then builds a single comma-separated channel list. In the offline check, the harmless input `join #books sample` queued `JOIN #books,#sample`. The client also created a buffer for the second name. On networks that allow channel creation or join notifications, a real key can become visible in a channel name, listing, membership record, or log. Visibility depends on the network. See `src/app.rs:2456–2492`. [IRC JOIN grammar](https://modern.ircdocs.horse/#join-message)

**Mitigation and acceptance.** Parse channel names and keys as separate parameters, preserve their positional association, and keep keys out of the sidebar, transcript, and editor history. If keyed joins are unsupported, reject the extra parameter before sending anything. Verify that a keyed join produces exactly one intended destination and that the sample key appears only in the wire key parameter.

### 03 · High · Group messages can be labeled private

**Evidence and impact.** `/msg` and `/query` always create a query buffer. The offline check observed a message addressed to `#books` in the outgoing queue, then found that the active buffer was a query. The query menu describes it as a “private conversation,” and follow-up messages go to the buffer’s stored name. A user can therefore send a reply to an entire channel from an interface that describes a private exchange. See `src/app.rs:2347,2532–2548,3069–3074,2254–2278`, `src/model.rs:174–179`.

A related incoming path checks only the first character against `CHANTYPES`. A status-targeted channel message can fall through to the query branch. On networks offering `STATUSMSG`, a reply to that target can reach multiple privileged channel members. Multiple-recipient targets also require an explicit audience in the UI. See `src/model.rs:349–353`, `src/app.rs:953–969,1576–1600`. [IRC STATUSMSG semantics](https://modern.ircdocs.horse/#statusmsg-parameter)

**Mitigation and acceptance.** Parse a destination once and carry its type through routing, rendering, and replies. Distinguish a single nickname, channel, status-targeted channel, and recipient list. Show the actual audience before sending. Only a validated single nickname may receive the private-conversation label. Cover both command entry and incoming-message paths.

### 04 · High · Distinct people or channels can share a buffer

**Evidence and impact.** The client applies RFC1459-style case folding globally and ignores the server’s `CASEMAPPING` advertisement. On a network that treats two permitted names as distinct, the client can reuse the first name’s buffer for the second. Incoming messages can be interleaved, and a reply typed into the reused buffer uses its stored recipient. This affects both private and channel conversations. It requires a network with a different mapping and names that collide under the client’s rule. See `src/util.rs:7–17`, `src/app.rs:316–369,913–918,962–969,1108–1126,1576–1600,2254–2275`. [IRC CASEMAPPING definitions](https://modern.ircdocs.horse/#casemapping-parameter)

**Mitigation and acceptance.** Store the negotiated mapping per server and use it consistently for buffers, members, own-nick checks, renames, and outgoing routes. Retain the actual wire target. On an unsupported mapping, avoid silently merging identities. Verify with two simultaneous networks using different mappings that names distinct on each network retain separate buffers and correct reply destinations.

### 05 · Medium · Network input can exhaust memory or starve the interface

**Evidence and impact.** The reader accumulates a `String` until a newline with no byte limit. Completed frames enter an unbounded UI queue. The UI drains that queue before drawing or polling the keyboard, and the outbound queue is also unbounded. New conversations and members grow independently of the 4,000-message cap per buffer. A faulty or hostile server can make the client unresponsive or exhaust memory. Ordinary peers have a narrower path through traffic the server permits. See `src/irc.rs:160–175,288–305,325–340`, `src/main.rs:61,218–230`, `src/app.rs:332–369,523–529,1759–1792`, `src/model.rs:8`.

The channel browser has a separate CPU problem. Each accepted LIST row rebuilds the filtered index across all rows received so far. With an empty filter, accepting *n* rows materializes *n(n+1)/2* index entries cumulatively. The 100,000-row cap therefore permits about five billion cumulative index entries. This calculation counts repeated work across the entire list. Peak allocation and wall-clock time were not measured. See `src/list.rs:56–59,73–104`, `src/app.rs:1553–1573`.

**Mitigation and acceptance.** Bound frames while reading bytes, independently accounting for IRCv3 tags. Bound queue capacities and retained state, and process a finite number of events or milliseconds before returning to user input. Update LIST selection incrementally or in batches. Verify oversized input is handled within a fixed memory budget, keyboard input stays responsive under sustained traffic, and doubling an unfiltered list costs approximately twice the ingestion work. The protocol permits up to 8,191 bytes of incoming tag section in addition to the ordinary 512-byte message. [IRCv3 tag size limits](https://ircv3.net/specs/extensions/message-tags#size-limit)

### 06 · Medium · Malformed server metadata can close every open session

**Evidence and impact.** The `005` capability handler finds the opening and closing delimiters of `PREFIX` independently, then uses an unchecked string slice. A malformed ordering gives an invalid range and Rust panics. This handler runs on the main event loop, so one connected server can end the entire process and interrupt unrelated connections. The panic hook restores the terminal but does not recover the app. See `src/app.rs:1343,1576–1595`, `src/main.rs:153–166,218–223`. Confirmed by source inspection. No crash was induced.

**Mitigation and acceptance.** Parse the value in order with checked string operations. Validate both the mode list and the symbol list before replacing the last known-good mapping. Parser tests should establish that any invalid field leaves the event loop running and preserves the previously accepted configuration.

### 07 · Medium · Inbound framing can lose data or disconnect on ordinary legacy text

**Evidence and impact.** `read_line` runs inside a `tokio::select!` alongside outgoing writes and keepalives. Tokio documents that cancellation can discard bytes from a partially read line. The remaining suffix can be fed to the IRC parser as a fresh frame. Separately, `read_line` requires valid UTF-8, and its error branch closes the connection. On networks that relay older encodings, a peer’s chat text can disconnect the recipient. See `src/irc.rs:288–301,325–358`, `src/app.rs:802–810`. [Tokio read_line contract](https://docs.rs/tokio/latest/tokio/io/trait.AsyncBufReadExt.html#method.read_line) · [IRCv3 UTF8ONLY](https://ircv3.net/specs/extensions/utf8-only)

**Mitigation and acceptance.** Give a bounded byte-oriented decoder ownership of partial frames across awaits, or use a dedicated reader task. Decode fields after framing. Preserve identity fields strictly and apply a defined fallback or discard policy to invalid message text. Verify fragmented valid frames survive interleaved writes exactly once, and one undecodable text message leaves the following valid frame and the connection intact.

### 08 · Medium · SASL state permits unsolicited credential responses and guest autojoin

**Evidence and impact.** The challenge handler checks for configured credentials but never checks that a SASL exchange is waiting for a response. Capability rejection and SASL failure both end negotiation. A subsequent welcome marks the client registered and triggers configured autojoins without checking authentication success. An error is shown for explicit failure, but automatic activity still proceeds if the server permits guest registration. This can expose channel interests or let someone speak under an unintended account state. The SASL payload also lacks the required 400-byte chunking and exact-boundary terminator, which can send long valid credentials down the failure path. See `src/app.rs:704–750,753–795,811–822,882–910,1522–1537`. [IRCv3 SASL exchange and framing](https://ircv3.net/specs/extensions/sasl-3.1)

**Mitigation and acceptance.** Track negotiation, challenge, response, authenticated, and failed states per connection. Honor challenges only in the expected state on verified TLS. Make configured authentication required by default, with an explicit guest policy if desired. Gate joins and ordinary sends until the required state is reached. Verify failure, skipped capability, repeated challenge, and long valid credential handling without logging secret material.

### 09 · Medium · Identity and privilege displays can carry another user’s information

**Evidence and impact.** A tagged chat message populates the sender’s cached account, while a later untagged message leaves that value in place. `account-notify` is handled but never requested. QUIT and disconnect clear channel membership while query metadata can survive. With `account-tag` negotiated, absence on a user message means the sender is unauthenticated. A reused nickname or logged-out peer can therefore appear to retain an earlier services account. See `src/app.rs:647–656,715–724,975–980,1041–1070,1319–1325,3069–3089`. [IRCv3 account-tag](https://ircv3.net/specs/extensions/account-tag) · [IRCv3 account-notify](https://ircv3.net/specs/extensions/account-notify)

There is also only one pending WHOIS record per server. Several replies update or finish it without checking the subject nickname. Overlapping requests can mix an account, operator flag, or secure-connection flag into the wrong popup. The channel role display has a similar freshness issue. A replacement NAMES entry with empty prefixes leaves old role prefixes intact, and the MODE parser hardcodes argument consumption. These are misleading client displays. Server-side permission checks still decide whether an operation succeeds. See `src/model.rs:278`, `src/app.rs:1200–1224,1458–1507,1655,1683–1736,1765–1780,2372–2376`.

**Mitigation and acceptance.** Track negotiated capabilities and clear stale identity on logout, quit, or reconnection. Distinguish unknown identity from a verified account. Match every WHOIS reply and completion to its server, session, and subject or serialize requests. Replace role snapshots even when empty and honor negotiated `CHANMODES`. Verify overlapping lookups, identity changes, and role removal never transfer trust labels to another user.

### 10 · Medium · Automatic version replies can loop

**Evidence and impact.** Incoming `PRIVMSG` and `NOTICE` share a CTCP handler. Both can trigger an automatic version reply, itself a `NOTICE`. Two clients with this behavior can keep exchanging responses. The client has no per-peer or per-server reply budget. Consequences include log noise, flood throttling, or disconnection, depending on the network. See `src/app.rs:823,924–949`. [IRC notice rules](https://modern.ircdocs.horse/#notice-message)

**Mitigation and acceptance.** Reply only to CTCP requests delivered through `PRIVMSG`, treat incoming `NOTICE` as terminal, and rate-limit automatic replies. Verify a valid version request gets one response and every version notice gets zero.

### 11 · Medium · Local secret handling leaves unnecessary copies and exposes clipboard helpers

**Evidence and impact.** The documented CLI accepts passwords directly as arguments. Depending on the shell, process permissions, and telemetry, these can enter history or process records. Inside the client, the editor stores up to 200 submissions before knowing which are sensitive. Service-authentication messages sent through `/msg` are also echoed into the query transcript, and channel MODE messages copy all arguments into chat history, including a key if the server supplied it. These are in-memory surfaces that can be recalled, displayed, or copied. See `src/config.rs:30–38,148–170`, `src/input.rs:37–46,133–150`, `src/app.rs:2131–2144,2532–2548,1250–1259`. [MITRE CWE-214](https://cwe.mitre.org/data/definitions/214.html)

The clipboard helper is launched by bare executable name and inherits the process environment, including `RIRC_PASS` when that fallback was used. A local actor who controls an earlier helper lookup location could receive clipboard contents and run code with the user’s rights. The prerequisite is control over local executable resolution. On Windows, the executable directory is searched before the system directory. See `src/config.rs:152–164`, `src/clipboard.rs:8–25,28–41,67–71`. [Rust child-process and executable resolution rules](https://doc.rust-lang.org/std/process/struct.Command.html#method.new)

**Mitigation and acceptance.** Prefer a masked prompt, protected input stream, or OS credential store. Classify sensitive commands before history insertion and local echo, and redact key-bearing mode arguments. Use the native clipboard API or a trusted absolute helper path and remove credential variables from child environments. Verify synthetic secret markers stay out of history, transcripts, and clipboard-helper environments.

### 12 · Low · Outgoing IRC frames lack a single validation boundary

**Evidence and impact.** The writer appends the line terminator to an arbitrary string. Ordinary paste flattens newlines, but startup channel values and the server-password argument reach command assembly without an equivalent check. A malformed value supplied through a launcher or imported configuration can change command boundaries. Chat chunking counts characters rather than encoded bytes, and `/msg` bypasses it. Valid multilingual text can exceed the server’s frame budget, causing truncation, rejection, or disconnection depending on server policy. See `src/config.rs:85–98,166–170`, `src/app.rs:576–584,685–695,908–910,2270–2275,2542–2548,3591–3595`, `src/irc.rs:301–308`, `src/util.rs:163–191`. [IRC message format and length handling](https://modern.ircdocs.horse/#message-format)

**Mitigation and acceptance.** Use one serializer that rejects embedded CR, LF, and NUL, validates middle parameters, accounts for command and target overhead, and enforces an encoded-byte budget. Route raw single-line commands through the same delimiter check. Verify every accepted command produces one legal frame and long Unicode messages split on valid character boundaries.

## Dependency advisories

The comparison used RustSec revision `10db6998b5d74ec438bcf0763125bd425a068a8f` and checked the patched and unaffected ranges for all 55 records with matching crate names. Four matched the locked versions. The two `lru` advisories describe specific unsafe APIs. The reviewed Ratatui layout-cache path uses `get_or_insert` with `(Rect, Layout)` keys. Inspection found no application path to either reported trigger. The other two records concern maintenance. [Pinned RustSec database](https://github.com/RustSec/advisory-db/tree/10db6998b5d74ec438bcf0763125bd425a068a8f)

| Locked dependency | Advisory and relevance | Mitigation |
|---|---|---|
| `ratatui 0.29.0 → lru 0.12.5` | Mutable iteration has an aliasing soundness issue. The affected range includes 0.12.5. [RUSTSEC-2026-0002](https://rustsec.org/advisories/RUSTSEC-2026-0002.html) | Move the parent dependency to a compatible release that resolves an `lru` version fixing both records. |
| `ratatui 0.29.0 → lru 0.12.5` | A panic during key destruction in `pop` can leave invalid pointers. No user-triggerable path was established in this client. [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html) | Verify `lru >=0.18.2` or removal of the dependency in the new lockfile. Ratatui 0.29 declares the 0.12 series, so an ordinary lockfile refresh alone cannot select 0.18.2. |
| Direct `chrono 0.4.45` | RustSec marks all releases unmaintained. The client uses it for local time and IRC timestamps. [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html) | Plan migration to a maintained time library and preserve server-time parsing, local timezone handling, and display behavior. |
| `ratatui 0.29.0 → paste 1.0.15` | Archived build-time procedural macro. RustSec identifies a maintenance risk. [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) | Prefer a maintained Ratatui dependency chain that removes or replaces `paste`. Verify with the resolved dependency tree. |

Lockfile entries appear at `Cargo.lock:82,402,521,575–576,593–597`. The advisory comparison covers crates recorded in this database. Native system libraries and the release executable have separate patch and provenance requirements.

## Remediation order

1. **Protect passwords and destinations before using this build for sensitive conversations.** Fix transport selection and registration, keyed joins, private/group labeling, and per-network name matching (01–04, 08). Promote the two failed conformance checks to permanent regression tests.
2. **Make the client tolerate untrusted network input.** Add byte-bounded, cancellation-safe framing and queue limits, then harden capability parsing, improve LIST ingestion, enforce output framing, and stop automatic notice loops (05–07, 10, 12).
3. **Make trust indicators and local secret handling reliable.** Correct account, WHOIS, and role lifetimes, and remove sensitive values from history and subprocesses (09, 11).
4. **Verify the release candidate.** Re-run the full suite where loopback is permitted, exercise TLS success and certificate rejection against a private test service, and repeat the pinned dependency audit after upgrades. Record the tested commit and resulting executable hash together.
