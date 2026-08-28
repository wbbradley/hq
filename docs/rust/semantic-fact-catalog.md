# HQ Rust semantic fact catalog

Status: normative reducer input and conflict specification

This catalog defines every first-release signed semantic family. Names describe product meaning and
are not wire type strings, Rust enum spellings, JSON tags, or SQL tables. Exact protocol mappings
belong to canonical fact v1 and remote-control v1.

The governing algebra is `docs/rust/causal-algebra.md`; normalized executable examples are in
`docs/rust/acceptance-scenarios.md`; product disposition comes from
`docs/rust/behavior-ledger.md`.

## Common rules

All catalog rows inherit these rules:

- IDs, installation/key/mailbox/account/message/project/resource/assignment/session/command IDs,
  timestamps, text, collections, addresses, locators, correlation, and sequence values are validated
  domain types.
- Every declared parent is a required causal dependency. Every typed authority reference is also a
  parent. A fact may add context parents, but cannot project until all declared parents are usable.
- `installation-private` facts are signed by and scoped to their own installation. `peer-addressed`
  facts have exact full sender/recipient addresses and mailbox capability. `account-addressed`
  facts name one human account and exact creator/device membership authority.
- Intrinsic shape or subject mismatch is `invalid`; a missing or currently unusable required parent
  is `unresolved`; available but insufficient authority is `unauthorized`; explicit aggregate
  ambiguity is `conflicted`.
- Canonical facts are retained permanently. `canonical-compacted-view` means exact canonical facts
  remain permanent while the named materialized view deterministically retains only selected
  winners/budgeted progress. Remote-control records are immutable durable audit until a later
  operations specification sets a bounded post-terminal archival policy; they never become project
  authority.
- Normalized observations always include fact decision/reason, blockers, authority roles,
  aggregate key, support IDs, and conflict participants in stable order. The last column lists the
  additional family-specific observations.

`none` in a parent or authority column means the empty set is valid for that root fact. `frontier`
always means every causal maximum of the named aggregate, never one sorted representative.

## Catalog

| ID | Semantic fact | Protocol class | Scope and signer | Required parents | Authority references | Intrinsic/domain validation | Unresolved behavior | Concurrent/conflict policy | Projection and exact support | Retention class | Additional normalized observations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| FCT-001 | InstallationDeclared | canonical | installation-private; declared installation root key | none | none | Installation UUID, signer key, and optional label are valid; signer equals declared root | none | Unequal roots for one installation UUID are a unique-root conflict; no active installation | Installation identity supported by the sole root | canonical-permanent | installation ID, root key, label, root-conflict set |
| FCT-002 | MailboxCreated | canonical | installation-private; installation root | InstallationDeclared | local-installation root | Mailbox ID is new on this installation; kind is human or agent; optional label is bounded | Missing installation root defers | Unequal creates for one mailbox conflict; one installation may have only one reserved human mailbox | Mailbox identity/kind/history supported by root and create | canonical-permanent | full mailbox address, kind, label, conflict state |
| FCT-003 | MailboxSessionBound | canonical | installation-private; installation root | MailboxCreated and relevant binding frontier | local-installation root | Provider/session pair is nonempty; mailbox is agent kind; immutable pair/subject relationship | Missing mailbox or frontier defers | Same provider/session bound to different mailboxes or incompatible binding on one mailbox conflicts; no reassignment | Permanent mailbox-to-provider-session history; conflicted pair is unusable | canonical-permanent | provider/session, mailbox, binding-conflict participants |
| FCT-004 | MailboxContextRecorded | canonical | installation-private; installation root | MailboxCreated and optional prior context frontier | local-installation root | Repository context values are typed display/search metadata and grant no authority | Missing mailbox or declared prior context defers | Grow-only history; concurrent contexts are all retained and current frontier is multivalue | Context history and frontier supported by mailbox/context chain | canonical-permanent | directory, repository identity, worktree, branch, context frontier |
| FCT-005 | PeerRouteSet | canonical | installation-private; local installation root | InstallationDeclared and complete peer-route frontier | local-installation root | Remote installation/key differ from local; label and relay hints are valid routing metadata | Missing local root or cited route frontier defers | Multivalue route updates conflict; any concurrent PeerRouteBlocked removes routing; a post-block set must descend from every block maximum | Directional peer route/key metadata, never mailbox authority | canonical-permanent | peer installation/key, labels/hints, route frontier, routable/conflicted |
| FCT-006 | PeerRouteBlocked | canonical | installation-private; local installation root | InstallationDeclared and complete peer-route frontier | local-installation root | Peer subject exists or is explicitly named; block contains no invented key authority | Missing route/root defers | Remove-wins against concurrent PeerRouteSet; later explicit set may restore only after all block maxima | Route becomes blocked while authorized history remains | canonical-permanent | peer ID, block frontier, blocked reason code |
| FCT-007 | MailboxAccessGranted | canonical | peer-addressed; target mailbox-owning installation root | MailboxCreated and any revoke maxima for the same prior lineage | mailbox-owner role | Exact target mailbox, grantee installation, and grantee signer key; owner signer/address match | Missing mailbox or cited revoke defers | Each grant is a distinct authority root; regrant is usable after revoke only when it descends from every relevant revoke maximum | Capability history and active grant candidate | canonical-permanent | grant ID, owner, grantee, mailbox, active/revoked state |
| FCT-008 | MailboxAccessRevoked | canonical | peer-addressed; target mailbox-owning installation root | Exact MailboxAccessGranted and complete observation/revoke frontier for that grant | mailbox-grant role naming exact grant | Subject payload equals grant; owner signer/address match; grant is not inferred by proximity | Missing grant/frontier defers | Remove-wins for concurrent/later actions using this grant; multiple revokes accumulate as maxima | Grant inactive for current routing; historical pre-revoke actions may remain | canonical-permanent | grant ID, revoke frontier, preserved action IDs, unauthorized action IDs |
| FCT-009 | MailboxActionObserved | canonical | peer-addressed; target mailbox-owning installation root | Exact grant, observed mailbox action, and complete observation frontier | mailbox-grant role naming exact grant | Observed action cites same grant and target mailbox; observer is receiver/owner | Missing grant/action/frontier defers | Grow-only observation history with exact maximal observation frontier | Proves observed action is causally before a later revoke that cites it | canonical-permanent | grant/action pair, observation frontier |
| FCT-010 | HumanAccountCreated | canonical | installation-private; creator installation root | InstallationDeclared | local-installation root | Account UUID, creator installation/key, and label match signer and local human mailbox | Missing installation root defers | Unequal roots for one account UUID conflict; no account or implicit creator is selected | Unique account root and creator membership | canonical-permanent | account ID, creator, label, root-conflict set |
| FCT-011 | HumanAccountSelected | canonical | installation-private; selecting installation root | HumanAccountCreated or matching active HumanDeviceAccepted; complete selection frontier | account-membership role and local-installation root | Selected account matches exact authority and selecting installation; arbitrary descendants do not qualify | Missing membership or selection frontier defers | Multivalue selection conflict; consumers requiring one default block until a later selection cites all maxima | Installation default account when selection is singular and membership active | canonical-permanent | selected candidates, selection frontier, active/default status |
| FCT-012 | HumanDeviceGranted | canonical | account-addressed; account creator root | HumanAccountCreated, creator membership authority, and target device revoke/regrant frontier | account-creator role | Exact account/creator/target installation/key/label/relay data; only creator grants | Missing root/frontier defers | Grants accumulate; a regrant after removal must descend from every maximal revoke for the device | Pending device invitation/grant history | canonical-permanent | device identity, grant ID, regrant lineage, relay hints |
| FCT-013 | HumanDeviceAccepted | canonical | account-addressed; invited target root key | Exact HumanDeviceGranted and any required revoke/regrant lineage | device-grant role naming exact grant | Payload exactly matches grant; signer is invited key; audience/account/addresses agree | Missing/mismatched grant or lineage defers; intrinsic signer mismatch is invalid | Acceptance is active only if it is a causal maximum not removed by a concurrent/later revoke | Active membership candidate and membership frontier | canonical-permanent | grant/accept IDs, device label/key, active/inactive reason |
| FCT-014 | HumanDeviceRevoked | canonical | account-addressed; account creator root | HumanAccountCreated, target grant, and every current acceptance/revoke maximum for that device | account-creator and target-grant roles | Creator/account/device identity match; revoke cannot target creator through a device grant | Missing root/grant/frontier defers | Remove-wins against concurrent or later-unrelated acceptance; all revoke maxima remain relevant | Device inactive; prior authorized causal history retained | canonical-permanent | device ID, revoke maxima, superseded acceptances |
| FCT-015 | QuestionAsked | canonical | installation-private, peer-addressed, or account-addressed; sender installation root | Address/scope authority and any chosen context parents; no self thread parent | local-installation, mailbox-grant, or account-membership role by scope | Root has full sender and allowed recipient/audience; typed presentation/correlation/text are valid; thread ID derives from fact ID | Missing authority/context defers; validated addressed content may be observed as incomplete | Distinct roots accumulate; stable public message-ID collision with unequal content conflicts | New question thread, inbox/action unit, message body and typed semantics | canonical-permanent | derived thread ID, sender/recipient/audience, open state, incomplete flag |
| FCT-016 | AsynchronousMessageSent | canonical | installation-private, peer-addressed, or account-addressed; sender installation root | Address/scope authority and chosen context parents | local-installation, mailbox-grant, or account-membership role by scope | Root addressing and typed text semantics match scope; no invented thread root | Missing authority/context defers; addressed incomplete observation allowed | Distinct roots accumulate; stable public message-ID collision conflicts | New asynchronous conversation/input; project purpose follows project rules | canonical-permanent | thread/message IDs, purpose, delivery audience, incomplete flag |
| FCT-017 | AnswerGiven | canonical | same allowed scopes; answering installation root | QuestionAsked thread root, relevant conversation parents, and scope authority | local-installation, mailbox-grant, or account-membership role | Thread root is a question; sender/recipient reverse the authorized question route; correlation is compatible | Missing root/authority defers; incomplete addressed observation does not answer thread | Answers form a grow-only set; no globally accepted winner; wait chooses first ready in canonical presentation order | Adds answer and answer/cancellation relations | canonical-permanent | thread ID, answer ID, relation to each cancellation, ready/consumed eligibility |
| FCT-018 | ThreadCancelled | canonical | same scope as question; original question sender installation root | QuestionAsked root, known thread frontier, and scope authority | same authority class as original question author | Target thread is a question authored by cancelling mailbox; reason is inert text | Missing root/frontier/authority defers | Cancellations accumulate independently of answers; no undo fact | Thread cancelled flag plus exact answer-before/after/concurrent relations | canonical-permanent | thread ID, cancellation IDs, relation matrix |
| FCT-019 | MessageArchived | canonical | installation-private or account-addressed; authorized human/control mailbox root | Target message and complete archive/restore frontier | local-installation or account-membership role | Target is a message causally preceding this fact and visible to the controlling mailbox/account | Missing target/frontier defers | Remove-wins against concurrent MessageRestored; later restore must descend from every archive maximum | Removes target from open/action view without deleting it | canonical-permanent | target ID, archived state, state frontier/support |
| FCT-020 | MessageRestored | canonical | installation-private or account-addressed; authorized human/control mailbox root | Target message and complete archive/restore frontier | local-installation or account-membership role | Target causally precedes restore and has no usable MessageRejected | Missing target/frontier defers | Loses to concurrent archive; opens only when descending from every maximal archive; reject is absorbing | Returns target to open view when singularly admitted | canonical-permanent | target ID, restored/open state, state frontier |
| FCT-021 | MessageRejected | canonical | installation-private or account-addressed; authorized human/control mailbox root | Target message and known message-state frontier | local-installation or account-membership role | Target causally precedes rejection; reason is inert text | Missing target/state frontier defers | Absorbing remove-wins; no first-release restore from rejection | Marks rejected and removes from open/action view permanently | canonical-permanent | target ID, rejected/archive state, rejection support |
| FCT-022 | HarnessActivityRecorded | canonical | installation-private or account-addressed; source agent installation root | Scope authority and prior logical-key frontier when superseding a snapshot | local-installation or account-membership role | Source is full agent mailbox; provider/session/operation/kind/item/runtime/positive sequence and bounded content are typed; never peer/public | Missing authority/key frontier defers; no incomplete actionable projection | Snapshots/progress use sequenced-snapshot rule; completed item at same semantic identity with unequal content conflicts; terminal items accumulate | Non-actionable conversation activity; reducer-selected winners only in compacted view | canonical-compacted-view | logical key, source sequence, winner/conflict, status/truncation, presentation position |
| FCT-023 | AgentNameClaimed | canonical | installation-private; installation root | Agent MailboxCreated and installation name-claim frontier | local-installation root | Lowercase slug is valid; mailbox is local agent; mailbox/name not already incompatibly claimed | Missing mailbox/frontier defers | Concurrent/different claims for one name or mailbox conflict; name becomes permanently reserved | Named-agent identity when unique; reservation/conflict always visible | canonical-permanent | name, mailbox, claim ID, reserved/active/conflicted |
| FCT-024 | AgentRetired | canonical | installation-private; installation root | AgentNameClaimed and complete agent/session/retirement frontier | local-installation root | Exact name/mailbox match; runtime quiescence is operational evidence, not payload authority | Missing claim/frontier defers | Absorbing remove-wins against concurrent/later session changes; name cannot be reused | Agent inactive/retired; history and names remain | canonical-permanent | name/mailbox, retirement ID, historical sessions |
| FCT-025 | ProviderSessionSelected | canonical | installation-private; installation root | AgentNameClaimed, matching MailboxSessionBound, and complete selection frontier | local-installation root | Session provider/ID and immutable repository/launch context match binding; agent not retired | Missing claim/binding/frontier defers | Multivalue selection conflict blocks runnable selection; later selection cites all maxima | Durable selected session and selection history, not runtime presence | canonical-permanent | agent, provider/session, context, selection candidates/frontier |
| FCT-026 | ProviderSessionRenamed | canonical | installation-private; installation root | AgentNameClaimed, MailboxSessionBound, and complete per-session rename frontier | local-installation root | Exact bound session; bounded display name or explicit clear; agent not retired when authoring | Missing claim/binding/frontier defers | Multivalue rename conflict exposes sorted candidates; later rename cites all maxima | Mutable display name/clear for historical session; no selection/runtime effect | canonical-permanent | session identity, name candidates, rename frontier |
| FCT-027 | ProjectCreated | canonical | account-addressed; immutable home installation root | Home InstallationDeclared, active human authority, and optional predecessor reference if known | project-home and active-human roles | Unique project/mailbox IDs, immutable home, name/brief, predecessor, typed desired resources, primary selection, initial open/closed state | Missing home/human/predecessor parent defers; absent optional predecessor is allowed only when not declared as parent | Unequal roots for one project ID conflict; overlapping active resources/global agent conflicts fail closed | Project identity, home, mailbox, desired resources, initial claims/lifecycle, head | canonical-permanent | project snapshot, head/root, predecessor, global conflicts |
| FCT-028 | ProjectOpened | canonical | account-addressed; project home root | ProjectCreated, exact previous head, active human authority, and relevant resource-claim observations | project-home and active-human roles | Prior state is unarchived closed; every desired resource can be claimed atomically | Missing previous head/authority/claim inputs defers | Home-linear fork conflicts; cross-project overlap marks claims/projects conflicted, never partial | Lifecycle open and one active claim per desired resource when conflict-free | canonical-permanent | new head, claim epochs, conflict set |
| FCT-029 | ProjectClosingStarted | canonical | account-addressed; project home root | Exact previous head and active human authority | project-home and active-human roles | Prior lifecycle open; stops new dispatch while retaining claims/assignment | Missing head/authority defers | Home-linear fork conflicts | Lifecycle closing; existing claims/assignment retained pending saga | canonical-permanent | head, closing state, active runtime/claim references |
| FCT-030 | ProjectClosed | canonical | account-addressed; project home root | Exact ProjectClosingStarted head, active human authority, and assignment-end/runtime-operation reference when applicable | project-home and active-human roles | Prior state closing; records forced flag and typed observed runtime outcome without claiming proof | Missing head/assignment reference defers | Home-linear fork conflicts | Closed, unassigned, all active claim epochs released; history preserved | canonical-permanent | head, forced flag, runtime observation, released epochs |
| FCT-031 | ProjectArchived | canonical | account-addressed; project home root | Exact previous head and active human authority | project-home and active-human roles | Project is closed and unarchived | Missing head/authority defers | Home-linear fork conflicts; archive is presentation removal over closed state | Archived flag true; project remains searchable/readable/permanent | canonical-permanent | head, archived state |
| FCT-032 | ProjectUnarchived | canonical | account-addressed; project home root | Exact previous head and active human authority | project-home and active-human roles | Project is archived and closed | Missing head/authority defers | Home-linear fork conflicts | Archived false; remains closed, unassigned, and without claims | canonical-permanent | head, visible-closed state |
| FCT-033 | ProjectMetadataUpdated | canonical | account-addressed; project home root | Exact previous head and active human authority | project-home and active-human roles | Nonempty display name and bounded optional brief; immutable fields unchanged | Missing head/authority defers | Home-linear fork conflicts | Replaces mutable name/brief only | canonical-permanent | head, name, brief |
| FCT-034 | ProjectResourceAdded | canonical | account-addressed; project home root | Exact previous head, active human authority, and relevant active-claim observations | project-home and active-human roles | New resource ID, path kind, home-qualified display and canonical locators with equal schemes, health observation; optional primary | Missing head/authority/claim inputs defers | Home-linear fork or cross-project canonical overlap fails closed; project-local overlap allowed | Adds desired resource and, when open/conflict-free, active claim epoch | canonical-permanent | head, resource, claim epoch, primary/conflict state |
| FCT-035 | ProjectResourceRemoved | canonical | account-addressed; project home root | Exact previous head, active human authority, and targeted resource | project-home and active-human roles | Resource exists; explicit assigned/force confirmation is semantic command evidence; no filesystem deletion | Missing head/resource/authority defers | Home-linear fork conflicts | Ends desired membership and active claim; chooses deterministic remaining primary or none | canonical-permanent | head, removed resource/claim, primary, warning/force state |
| FCT-036 | ProjectResourceReplaced | canonical | account-addressed; project home root | Exact previous head, active human authority, old resource, and relevant active-claim observations | project-home and active-human roles | Old exists; new ID plus display/canonical identity is distinct and valid; external saga outcome is reconciled | Missing head/resource/claim inputs defers | Atomic global conflict rule; never exposes both old released and new unclaimed as success | Adds new desired/claim and ends old in one authoritative transition; preserves primary intent | canonical-permanent | head, old/new resources and claim epochs, conflict state |
| FCT-037 | ProjectPrimaryResourceChanged | canonical | account-addressed; project home root | Exact previous head, active human authority, and selected desired resource | project-home and active-human roles | Resource belongs to project and is path kind | Missing head/resource/authority defers | Home-linear fork conflicts | Changes primary path only; no resource reorder or runtime relocation | canonical-permanent | head, primary resource ID |
| FCT-038 | ProjectResourceHealthObserved | canonical | account-addressed; project home root | Exact previous head and selected desired resource | project-home role; active-human role when observation accompanies a mutation | Typed health/details/check time from resource adapter; observation grants no lifecycle transition | Missing head/resource defers | Home-linear log; repeated equal observation still audit-valid but notice coalescing is derived | Updates health observation; may support one material-change/recovery notice | canonical-permanent | resource health/details/check time, material-change classification |
| FCT-039 | ProjectAssignmentConfiguring | canonical | account-addressed; project home root | Exact previous head, active human authority, open project, and unique local agent claim | project-home and active-human roles | Project open/unassigned; agent active, local to home, and globally unassigned; new assignment ID and provider intent; no provider session exists yet | Missing head/agent/authority defers | Home fork or global agent-cardinality conflict marks all competing assignments non-runnable | Adds a session-free configuring assignment epoch; project retains claims | canonical-permanent | head, assignment/agent/provider intent, global conflicts |
| FCT-040 | ProjectAssignmentRunnable | canonical | account-addressed; project home root | Exact previous head, matching session-free configuring intent, scoped project thread, and acknowledged provider session binding | project-home role and activation operation correlation | Assignment/agent/provider intent match; thread immutable scope is project; provider session and launch directory are acknowledged | Missing head/assignment/thread defers | Home fork or global agent conflict blocks runnable projection | Binds the session and makes the assignment runnable with one selected scoped thread | canonical-permanent | head, assignment, agent, project/provider thread, launch directory |
| FCT-041 | ProjectAssignmentBlocked | canonical | account-addressed; project home root | Exact previous head and matching active assignment | project-home role | Typed blocked cause and diagnostic fields; assignment matches | Missing head/assignment defers | Home-linear fork conflicts | Assignment remains owned but non-runnable/blocked for explicit resolution | canonical-permanent | head, assignment, blocked cause |
| FCT-042 | ProjectAssignmentEnded | canonical | account-addressed; project home root | Exact previous head, matching assignment, and active human authority for force | project-home and active-human-on-force roles | Assignment matches; forced/runtime observation do not claim external cessation | Missing head/assignment/force authority defers | Home fork conflicts; ending one side may resolve global cardinality conflict | Ends assignment epoch, preserves suggested agent/history, removes current assignment | canonical-permanent | head, ended epoch, forced/runtime observation |
| FCT-043 | ProjectInputAccepted | canonical | account-addressed; project home root | Exact previous head, project-addressed QuestionAsked or AsynchronousMessageSent, project-home root, and active account authority | previous-state, project-home, and account-membership roles | Message account/project/mailbox match the immutable project; public ID/fact ID unique; sequence is exactly predecessor sequence plus one | Missing head/message/authority defers | Home fork conflicts; duplicate input ID/fact or sequence collision conflicts | Adds authoritative project input sequence; pending until valid dispatch | canonical-permanent | head, message/fact ID, sequence, pending state |
| FCT-044 | ProjectInputDispatched | canonical | account-addressed; project home root | Exact previous head, ProjectInputAccepted, current runnable assignment, and selected scoped thread | project-home role | Input/sequence undispatched; assignment/agent/thread/external session exactly match | Missing head/acceptance/assignment/thread defers | Home fork, duplicate message dispatch, or duplicate sequence conflicts; at most once | Binds input to assignment/agent/thread and marks dispatch accepted by HQ runtime boundary | canonical-permanent | head, input sequence, immutable dispatch attribution |
| FCT-045 | ProjectOutputRecorded | canonical | account-addressed; project home root authoring for exact agent mailbox | Originating dispatch or assignment/thread provenance, project mailbox, and active human audience authority | project-home and exact output-binding roles | Stable output ID and complete typed message/provenance collide on any changed field; assignment/thread/agent match captured binding | Missing provenance/audience defers | Identical retry deduplicates; changed same-ID output conflicts; current and late outputs both retained | Adds project conversation output; marks current or late-from-inactive assignment without changing lifecycle | canonical-permanent | output ID, project/agent/thread/assignment, current/late classification, collision |
| FCT-046 | RemoteProjectCommandRequested | remote-control | account-addressed; active human-device root | Active membership authority, prior local command frontier, and observed ProjectCreated/head for post-create commands | active-human role | Stable command ID/digest, target home/project, optional expected head, typed operation/body; absent head is valid only for creation | Missing membership or a cited project/head defers local transmission/projection | Same ID with different digest conflicts; competing expected-head commands remain distinct and home serializes/rejects | Adds durable accepted/queued remote-command view only; never mutates project | control-permanent | command ID/digest, optional expected head, accepted/queued stage |
| FCT-047 | RemoteProjectCommandReceipt | remote-control | account-addressed; target project-home root | Exact RemoteProjectCommandRequested, project-home root, and home-observed project head when one exists | project-home role | Home/project/command/digest match request; received head is explicit and absent for creation before the project exists | Missing request, home authority, or a present head defers | Multiple identical receipts deduplicate; unequal receipt for same command conflicts | Advances command to received and records authoritative observed head or creation absence; no project mutation | control-permanent | command ID, optional received head/time, receipt conflict |
| FCT-048 | RemoteProjectCommandOutcome | remote-control | account-addressed; target project-home root | Exact request and receipt; committed canonical project head/facts when outcome is committed | project-home role | Terminal outcome is committed or rejected with typed definite/unknown runtime result; committed head exists and descends expected history | Missing request/receipt/committed fact defers | Unequal terminal outcomes for one command conflict and no terminal winner is reported | Advances remote-command view to committed/rejected plus runtime result; canonical project facts remain sole state authority | control-permanent | command ID, terminal stage, current head, typed result/error/uncertainty |

## Installation-local control and identity conflicts

Installation-private does not mean unsigned. `InstallationDeclared`, mailbox, peer, selection, and
agent/session facts are signed by the installation root and enter the same immutable fact set and
batch reducer. Runtime leases, process presence, environments, filesystem checks, mutation
receipts, and drafts are not semantic facts in this catalog.

Unique roots fail closed. A duplicate ID with identical fact content deduplicates; two unequal
roots for the same installation, mailbox, account, project, name, provider/session binding, stable
message ID, stable output ID, or remote command ID produce a normalized collision/conflict. No
timestamp, receipt order, or lexical fact-ID choice hides it.

Mutable local choices use frontier-complete multivalue registers. A selection or rename that sees
two maxima resolves them by citing both. Until then, user interfaces can display all candidates,
but the node cannot launch a worker from an ambiguous selected session or send account traffic from
an ambiguous default account.

## Capability revocation and observations

Mailbox access is directional. The owner of mailbox `m` grants installation/key `k` the right to
send actions to `m`; a peer route is necessary for transport but never substitutes for the grant.
Every action names the exact grant as the `mailbox-grant` authority.

An owner observation is a signed causal acknowledgement of one accepted action. A revoke cites the
grant and the complete observation/revoke frontier. Therefore an observed action is structurally
and usefully before the revoke. When a later revoke arrives at a replica:

- observed/pre-revoke actions remain projected;
- an action concurrent with the revoke retracts to `unauthorized`;
- an action after the revoke that still cites the old grant is `unauthorized`; and
- a later action may project only by citing a new grant whose lineage descends from every maximal
  revoke.

The receiver never infers observation from relay acceptance. A later usable action from the peer
can establish peer receipt of its own parent for delivery presentation, but cannot sign the
mailbox-owner observation required for revoke preservation.

## Human account membership

The creator root is permanent first-release administration authority. A non-creator device becomes
active only through an exact creator grant followed by target-key acceptance. The membership
frontier contains all causal-maximal matching acceptances and revokes for a device.

Active membership is derived as follows:

1. the account has one usable root;
2. grant and acceptance payloads and keys match exactly;
3. the acceptance descends from the grant;
4. no maximal revoke is concurrent with or after that acceptance; and
5. after any revoke, a replacement grant and acceptance descend from every maximal revoke.

Account actions cite the creator root or one active maximal acceptance for the named account.
Membership in another account, peer routing, an arbitrary causal descendant, a historical
nonmaximal acceptance, or a relay recipient tag grants nothing. One account fact may fan out in
multiple encrypted wrappers, but its fact ID and decision are identical at every device.

## Conversation and activity conflicts

Question and asynchronous-message roots derive their causal thread identity from the canonical
fact ID. Answer and cancellation always cite that root. Answers and cancellations are independent
grow-only sets; the reducer reports every pair as `before`, `after`, or `concurrent` using usable
reachability.

Message open state is a register over archive/restore maxima plus an absorbing rejection bit:

- any usable rejection means `rejected=true` and `open=false` permanently;
- otherwise any maximal archive means `open=false`;
- otherwise a later frontier-complete restore means `open=true`; and
- canonical message and state facts remain available for audit in every case.

Activity is keyed by source installation/mailbox plus provider, session, operation, kind, and
optional item. Runtime and positive source sequence distinguish writer lifetimes and order. Plan,
diff, running state, and progress are sequenced snapshots. Completed command/file/tool and terminal
operation items remain logical history. A higher sequence selects a concurrent snapshot only within
the exact source/runtime namespace; conflicting runtimes or equal-sequence unequal payloads are
reported rather than hidden. Activity never affects inbox, unread, reply, archive, draft, delivery,
action-unit, or final-answer state.

## Project state and global safety

Each project has one immutable home and one home-linear fact history. Creation has no previous head;
every later project fact cites exactly the current head. A replica receiving a child before its
head defers it. Sibling children of one head are a permanent first-release fork: the projection
stops at the common head, reports both branches, and does not select by authored time or fact ID.

The home-linear rule does not by itself prove cross-project safety. Reduction also computes these
global projections per home:

- active path claims conflict when canonical paths are equal or ancestor/descendant across open
  projects;
- project-local overlapping paths are permitted;
- an agent has at most one active assignment and a project at most one active agent; and
- a runnable assignment has exactly one selected thread immutably scoped to its project and agent.

If valid facts expose a cross-project overlap or double assignment, every conflicting participant
is marked non-runnable/non-claimable and the conflict is observable. Ending/releasing one relation
may retract the conflict and leave one participant active. Store serialization should prevent these
states during normal authoring, but rebuild correctness cannot assume the store never failed.

Project health is observation, not lifecycle authority. Close, force close, takeover, and resource
removal record explicit decisions and observations, but no signed fact proves an arbitrary external
process stopped or a filesystem was changed. External saga checkpoints remain durable operational
state; only a successful home fact changes authoritative project state.

Project input acceptance establishes one contiguous home sequence. Dispatch binds exactly one
accepted input to the then-runnable assignment, agent, and selected thread. Output keeps immutable
provenance. Output from an ended/replaced assignment remains conversation history marked late and
cannot mutate project state or regain dispatch authority.

## Remote-control isolation

Remote-control records are signed, causal, immutable, and reduced for command visibility, but are
not project transition facts. A valid `RemoteProjectCommandRequested` means an active human device
asked the immutable home to decide a command at an expected head. It does not mean the home received
it, committed it, or completed a runtime effect.

Only home-signed canonical project facts advance project state. A receipt proves home receipt and
the head it observed. A committed outcome cites the request, receipt, and committed canonical head;
a rejected outcome records the current head and typed reason. Runtime-crossing commands also expose
definite success/failure or explicit uncertainty. Relay acceptance and local queueing are earlier
operational observations and cannot be promoted into these semantic stages.

## Catalog completeness rule

Adding a first-release semantic family requires a new `FCT-*` row, conflict rule, normalized
observation, and at least one positive and one adverse named acceptance scenario. Changing a row's
scope, authority, conflict, projection, or retention is a reviewed semantic change even when the
wire and database have not yet been implemented.
