# HQ protocol payload mapping v1

Status: normative exhaustive DTO registry

This document is the only mapping between protocol v1 DTOs and the 48 semantic fact families. The
numeric family code, property names, object order, enum strings, and nested shapes are wire
vocabulary owned by `hq-protocol`. Rust type names below identify conversion targets only; their
field names and layouts are not wire contracts. Go values are not accepted aliases.

## Common envelope mapping

| Wire field | Semantic target | Conversion |
| --- | --- | --- |
| `p` plus `f` | protocol class plus `FactKind` | exact registry row below; no string enum inference |
| verified event `id` | `SemanticFact.id` | 32 decoded bytes in a typed canonical/control fact-ID namespace |
| `author` plus verified `pubkey` | `SemanticFact.author` | installation ID plus BIP-340 signing key; neither may override the other |
| `time` | `SemanticFact.authored_at` | nonnegative Unix milliseconds, with outer-second agreement |
| `scope` | `SemanticFact.scope` | exact tagged-array conversion from the owning protocol spec |
| `parents` | `CausalReferences.parents` | sorted unique typed refs; namespace erased only after protocol isolation is proved |
| `auth` | `CausalReferences.authorities` | wire role string to closed `AuthorityRole`; exact ref must also be a parent |
| `body` | `SemanticPayload` | family-specific owned DTO conversion below |

The conversion layer constructs owned values. It does not borrow a generic JSON tree as domain
state. Protocol DTOs may resemble semantic fields but are separate types and validation phases.

## Primitive and nested conversion registry

- `hex` is exact 32-byte lowercase hex. Its semantic ID/key type is selected by the property, never
  inferred from length. A command digest is also opaque 32 bytes.
- `milliseconds` maps to signed semantic `Timestamp`; v1 accepts only 0 through
  9,223,372,036,854,775,807. `sequence` maps to `NonZeroU64` and is 1 through
  18,446,744,073,709,551,615.
- `short`, `content`, provider/session, and locator texts use the named limits in canonical v1;
  required text is nonempty after decoding. No trimming or Unicode normalization occurs.
- `optional` always means the property is present and its value is either `null` or the stated DTO.
- `installation-address` is exactly `{"installation":hex,"signing":hex}` and maps respectively to
  installation ID and signing key.
- `mailbox-address` is exactly `{"installation":hex,"mailbox":hex}`.
- `locator` is exactly `{"scheme":scheme,"value":locator-text}`. Scheme strings `git`,
  `worktree`, `container`, and `opaque` map respectively to Git repository, working tree,
  container, and opaque semantic schemes.
- `context` is exactly
  `{"directory":locator,"repository":optional-locator,"worktree":optional-locator,"branch":optional-short}`.
- `operation` is exactly `{"provider":provider-text,"session":session-text,"id":hex}`.
- `message` is exactly
  `{"id":hex,"sender":mailbox-address,"recipient":optional-mailbox-address,"body":content,"purpose":purpose,"presentation":presentation,"correlation":optional-operation,"project":optional-hex}`.
  Purpose strings are `question`, `asynchronous`, and `project-output`; presentation strings are
  `message`, `final-answer`, and `status`.
- `resource` is exactly `{"id":hex,"locator":locator,"health":health}`. Health is `unknown`,
  `healthy`, `degraded`, or `unavailable`.
- `binding` is exactly
  `{"assignment":hex,"agent":hex,"provider":provider-text,"session":session-text}`.
- `activity-status` is exactly `{"state":state}` except failed, which is
  `{"state":"failed","code":short}`. Other states are `snapshot`, `running`, `succeeded`, and
  `interrupted`.
- `runtime` is exactly `{"state":"succeeded"}`,
  `{"state":"failed","code":short}`, or `{"state":"uncertain","code":short}`.

Nested object member order is the displayed order. Every property is required even when optional.
Unknown or duplicate properties and unknown enum spellings are rejected.

## Exhaustive family registry

In the body-shape column, properties appear in required canonical order. `[]` is an ordered bounded
array. The final column records body-to-semantic conversion and intrinsic agreement beyond basic
type validation. Required historical parents and authority sufficiency remain governed by the
semantic catalog and reducer; the protocol accepts only role names applicable to the family.

| Catalog | Code | Semantic target | Protocol | Exact `body` shape | Intrinsic conversion and agreement |
| --- | ---: | --- | --- | --- | --- |
| FCT-001 | 1 | InstallationDeclared | hq/canonical | `{"installation":hex,"signing":hex,"encryption":hex,"label":optional-short}` | fields map to installation ID, signing key, encryption key, label; installation equals author and local scope, signing equals verified pubkey |
| FCT-002 | 2 | MailboxCreated | hq/canonical | `{"mailbox":hex,"kind":mailbox-kind,"label":optional-short}` | mailbox ID; `human` or `agent`; label; local author scope |
| FCT-003 | 3 | MailboxSessionBound | hq/canonical | `{"mailbox":hex,"provider":provider-text,"session":session-text}` | mailbox ID and nonempty provider/session pair |
| FCT-004 | 4 | MailboxContextRecorded | hq/canonical | `{"mailbox":hex,"context":context}` | mailbox ID and repository-context DTO |
| FCT-005 | 5 | PeerRouteSet | hq/canonical | `{"peer":installation-address,"encryption":hex,"label":optional-short,"relays":[locator]}` | peer address, encryption key, label, ordered unique relay hints; peer differs from author |
| FCT-006 | 6 | PeerRouteBlocked | hq/canonical | `{"peer":hex,"reason":short}` | peer installation ID and domain error code |
| FCT-007 | 7 | MailboxAccessGranted | hq/canonical | `{"grant":hex,"mailbox":mailbox-address,"grantee":installation-address}` | exact grant, owned mailbox, and grantee addresses; peer scope is the exact mailbox |
| FCT-008 | 8 | MailboxAccessRevoked | hq/canonical | `{"grant":hex,"mailbox":mailbox-address,"grantee":hex}` | grant ID, mailbox, grantee installation ID; peer scope is exact mailbox |
| FCT-009 | 9 | MailboxActionObserved | hq/canonical | `{"grant":hex,"action":hex}` | grant ID and canonical action fact ID |
| FCT-010 | 10 | HumanAccountCreated | hq/canonical | `{"account":hex,"creator":installation-address,"label":optional-short}` | account ID, creator, label; creator equals verified author |
| FCT-011 | 11 | HumanAccountSelected | hq/canonical | `{"account":hex}` | account ID agrees with selected membership authority |
| FCT-012 | 12 | HumanDeviceGranted | hq/canonical | `{"account":hex,"grant":hex,"device":installation-address,"label":optional-short,"relays":[locator]}` | account/grant/device IDs, label, ordered unique relay hints; account scope agrees |
| FCT-013 | 13 | HumanDeviceAccepted | hq/canonical | `{"account":hex,"grant":hex,"device":installation-address}` | exact grant/device/account; verified author equals invited device address; account scope agrees |
| FCT-014 | 14 | HumanDeviceRevoked | hq/canonical | `{"account":hex,"grant":hex,"device":hex}` | account, target grant, and device installation ID; account scope agrees |
| FCT-015 | 15 | QuestionAsked | hq/canonical | `message` | maps all message fields; purpose must be `question`; root has no thread body field; scope agrees with sender/recipient/audience |
| FCT-016 | 16 | AsynchronousMessageSent | hq/canonical | `message` | maps all message fields; purpose must be `asynchronous`; scope agrees with sender/recipient/audience |
| FCT-017 | 17 | AnswerGiven | hq/canonical | `{"thread":hex,"message":message}` | thread ID and all message fields; message route reverses the cited question's authorized route |
| FCT-018 | 18 | ThreadCancelled | hq/canonical | `{"thread":hex,"reason":optional-content}` | question thread ID and inert optional reason |
| FCT-019 | 19 | MessageArchived | hq/canonical | `{"message":hex}` | target message ID |
| FCT-020 | 20 | MessageRestored | hq/canonical | `{"message":hex}` | target message ID |
| FCT-021 | 21 | MessageRejected | hq/canonical | `{"message":hex,"reason":short}` | target message ID and domain error code |
| FCT-022 | 22 | HarnessActivityRecorded | hq/canonical | `{"source":mailbox-address,"operation":operation,"item":optional-short,"kind":activity-kind,"logical_key":short,"runtime":short,"sequence":sequence,"occurred_at":milliseconds,"status":activity-status,"content":content,"truncated":boolean}` | all activity fields map directly; activity kind is `status`, `progress`, `plan`, `diff`, or `completed-item`; source belongs to author and scope; runtime and sequence form the writer order |
| FCT-023 | 23 | AgentNameClaimed | hq/canonical | `{"agent":hex,"mailbox":hex,"name":short}` | durable agent/mailbox IDs and validated lowercase slug |
| FCT-024 | 24 | AgentRetired | hq/canonical | `{"agent":hex,"mailbox":hex}` | durable agent and mailbox IDs |
| FCT-025 | 25 | ProviderSessionSelected | hq/canonical | `{"agent":hex,"mailbox":hex,"provider":provider-text,"session":session-text,"context":context}` | agent/mailbox, exact provider/session, and repository context |
| FCT-026 | 26 | ProviderSessionRenamed | hq/canonical | `{"agent":hex,"provider":provider-text,"session":session-text,"display":optional-short}` | agent and exact provider/session identity plus optional display name |
| FCT-027 | 27 | ProjectCreated | hq/canonical | `{"project":hex,"mailbox":hex,"home":hex,"name":short,"brief":optional-content,"predecessor":optional-hex,"resources":[resource],"primary":optional-hex,"state":initial-state}` | all project root fields; `open` or `closed`; home equals author installation; primary names exactly one listed resource; resource IDs are unique |
| FCT-028 | 28 | ProjectOpened | hq/canonical | `{"project":hex}` | project ID; exact previous-state/project-home relations are envelope refs |
| FCT-029 | 29 | ProjectClosingStarted | hq/canonical | `{"project":hex}` | project ID |
| FCT-030 | 30 | ProjectClosed | hq/canonical | `{"project":hex,"forced":boolean,"runtime":optional-runtime}` | project ID, explicit force evidence, optional runtime observation |
| FCT-031 | 31 | ProjectArchived | hq/canonical | `{"project":hex}` | project ID |
| FCT-032 | 32 | ProjectUnarchived | hq/canonical | `{"project":hex}` | project ID |
| FCT-033 | 33 | ProjectMetadataUpdated | hq/canonical | `{"project":hex,"name":short,"brief":optional-content}` | project ID, nonempty display name, optional brief |
| FCT-034 | 34 | ProjectResourceAdded | hq/canonical | `{"project":hex,"resource":resource,"primary":boolean}` | project and new unique resource; boolean maps to make-primary intent |
| FCT-035 | 35 | ProjectResourceRemoved | hq/canonical | `{"project":hex,"resource":hex,"force":boolean}` | project/resource IDs and explicit force evidence |
| FCT-036 | 36 | ProjectResourceReplaced | hq/canonical | `{"project":hex,"old_resource":hex,"resource":resource}` | project, old resource ID, and complete new resource DTO |
| FCT-037 | 37 | ProjectPrimaryResourceChanged | hq/canonical | `{"project":hex,"resource":hex}` | project and selected desired resource IDs |
| FCT-038 | 38 | ProjectResourceHealthObserved | hq/canonical | `{"project":hex,"resource":hex,"health":health,"details":optional-content,"checked_at":milliseconds}` | exact project/resource, typed health, optional details, observation time |
| FCT-039 | 39 | ProjectAssignmentConfiguring | hq/canonical | `{"project":hex,"binding":binding}` | project and complete immutable assignment binding |
| FCT-040 | 40 | ProjectAssignmentRunnable | hq/canonical | `{"project":hex,"binding":binding,"thread":hex,"launch_directory":locator,"activation":operation}` | project/binding/thread, working directory, and exact activation correlation |
| FCT-041 | 41 | ProjectAssignmentBlocked | hq/canonical | `{"project":hex,"assignment":hex,"cause":short}` | project/assignment and stable blocked error code |
| FCT-042 | 42 | ProjectAssignmentEnded | hq/canonical | `{"project":hex,"assignment":hex,"forced":boolean,"runtime":optional-runtime}` | project/assignment, force evidence, optional runtime observation |
| FCT-043 | 43 | ProjectInputAccepted | hq/canonical | `{"project":hex,"message":hex,"input_fact":hex,"sequence":sequence}` | project/public message/canonical input fact IDs and positive contiguous sequence |
| FCT-044 | 44 | ProjectInputDispatched | hq/canonical | `{"project":hex,"message":hex,"sequence":sequence,"dispatch":hex,"binding":binding,"thread":hex}` | exact accepted input identity and sequence, stable dispatch ID, captured binding/thread |
| FCT-045 | 45 | ProjectOutputRecorded | hq/canonical | `{"project":hex,"output":hex,"dispatch":hex,"binding":binding,"thread":hex,"message":message}` | complete provenance and message; message ID equals output, purpose is `project-output`, and message project equals project |
| FCT-046 | 46 | RemoteProjectCommandRequested | hq/control | `{"command":hex,"digest":hex,"project":hex,"target_home":hex,"expected_head":hex,"operation":operation,"body":content}` | stable command/digest, project/home/head, exact operation and inert body; target home equals control scope |
| FCT-047 | 47 | RemoteProjectCommandReceipt | hq/control | `{"command":hex,"digest":hex,"project":hex,"received_head":hex,"received_at":milliseconds}` | fields match cited request; received head is a canonical project head; author is scoped home |
| FCT-048 | 48 | RemoteProjectCommandOutcome | hq/control | `{"command":hex,"digest":hex,"project":hex,"result":remote-result,"runtime":optional-runtime}` | command tuple matches request; result is committed head or rejected code; author is scoped home |

## Remote result DTO

`remote-result` is exactly one of these member-ordered objects:

- `{"state":"committed","head":hex}` maps to `RemoteCommandResult::Committed`; the exact
  canonical head is also a typed `c` parent.
- `{"state":"rejected","code":short}` maps to `RemoteCommandResult::Rejected`.

No boolean success shorthand, null result, error object, or Go command-result alias is accepted.

## Scope and authority applicability

The protocol layer rejects a recognized role on a family where it has no possible semantic use.
The allowable role sets below are maxima; the semantic catalog determines which are required in a
particular causal situation.

| Families | Allowed scope | Applicable authority roles |
| --- | --- | --- |
| 1 | local | none |
| 2–6, 23–26 | local | `local-installation` |
| 7 | peer | `mailbox-owner` |
| 8–9 | peer | `mailbox-grant` |
| 10 | local | `local-installation` |
| 11 | local | `local-installation`, `account-membership` |
| 12 | account | `account-creator` |
| 13 | account | `device-grant` |
| 14 | account | `account-creator`, `device-grant` |
| 15–18 | local, peer, or account | respectively `local-installation`, `mailbox-grant`, or `account-membership` |
| 19–22 | local or account | respectively `local-installation` or `account-membership` |
| 27 | account | `project-home`, `account-membership`, `active-human` |
| 28–37, 39 | account | `previous-state`, `project-home`, `account-membership`, `active-human` |
| 38 | account | `previous-state`, `project-home`, and optional matching `account-membership`/`active-human` for a human mutation |
| 40–41 | account | `previous-state`, `project-home`, `assignment` |
| 42 | account | `previous-state`, `project-home`, `assignment`, and matching `account-membership`/`active-human` when forced |
| 43 | account | `previous-state`, `project-home` |
| 44 | account | `previous-state`, `project-home`, `assignment`, `dispatch` |
| 45 | account | `previous-state`, `project-home`, `dispatch`, `assignment`, `output-binding` |
| 46 | control | `account-membership`, `active-human`, `project-home` |
| 47–48 | control | `project-home`, `request`; matching `account-membership`/`active-human` may preserve requesting-account context |

An allowed role is not evidence that the cited fact actually confers it. The reducer checks exact
family, subject, ancestry, active frontier, and causal relation. Extra context parents need no role
and are allowed within the parent limit when their namespace is legal.

## Completeness and evolution

The registry has exactly one row for every FCT catalog family. Canonical codes are the closed range
1 through 45; control codes are the closed range 46 through 48. The ranges are disjoint even though
the top-level discriminator already distinguishes them. A new family requires a semantic catalog
entry, owned DTO, mapping row, positive vector, adverse vector, and version-compatibility decision.
Renaming a Rust field alone does not alter this document; changing a wire property, order, enum, or
meaning is a protocol change.
