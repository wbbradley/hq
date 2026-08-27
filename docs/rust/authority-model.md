# HQ Rust authority reduction model

Status: implemented pure-domain contract

This document maps the installation, peer, mailbox-capability, and human-account rules in
`semantic-fact-catalog.md` to the pure `hq-reducer::AuthorityReducer`. The reducer consumes only a
complete `FactSet`, its causal graph, and explicit `AuthorityPolicy`; it does not inspect encoded
events, receipt order, relays, storage, configuration files, process state, or clocks.

## Typed authority roles

`AuthorityRole` uses the catalog vocabulary directly. Local installation, mailbox owner, mailbox
grant, account creator, device grant, account membership, previous state, project home, active
human, assignment, dispatch, request, and output binding are distinct map keys. A fact that cites an
ordinary parent but omits the required role is unauthorized. A role that cites the wrong payload
family, subject, audience, or signer is never repaired by another ancestor.

The explicit local policy contains the installation ID and its reserved human mailbox ID. It is a
semantic input to batch reduction, not ambient configuration. The local reserved human mailbox
must use that exact ID; remote installation histories still enforce their own unique human-mailbox
cardinality without becoming local defaults.

## Normalized authority projections

The reducer emits typed keys and values for:

- unique installation and installation-qualified mailbox roots;
- directional peer routes keyed by local installation and remote installation;
- directional mailbox capabilities keyed by grant ID;
- unique human-account creator roots;
- device membership keyed by account and installation; and
- the local account-selection register.

Every value is rebuildable and its report support expands through usable causal parents. Unique
root conflicts project no active root. Route and selection registers retain all causal maxima;
their normalized state is blocked, conflicted, or singular rather than choosing by time or ID.

## Mailbox capability history

A mailbox grant is signed by the mailbox owner and cites the exact mailbox creation under the
`MailboxOwner` role. An addressed action is authorized only when its `MailboxGrant` role cites a
grant for the exact target mailbox and exact author installation/key. Peer routing is deliberately
absent from this test: it enables transport but grants no mailbox authority.

Every known usable revoke of the cited grant is evaluated at the action's causal point. The action
survives when it is usefully before the revoke, normally through an owner-signed observation that
the revoke cites. A concurrent action or an action descending from the revoke while citing the old
grant is unauthorized. A replacement grant has a new grant ID and must descend from every maximal
revoke for the same mailbox and grantee; a partial lineage remains unauthorized and its dependants
remain unresolved.

Capability projections retain the grant, current revoke frontier, owner-observed action IDs, and
an active flag. Adding a revoke therefore grows knowledge while retracting the active capability
and any concurrent old-grant action projection.

## Human membership history

The account creator root is authorized by the creator's installation root. A non-creator device
requires an exact `HumanDeviceGranted` fact followed by a target-key-signed
`HumanDeviceAccepted`; the acceptance's account, grant ID, installation, and signing key must match
the grant exactly.

Membership uses a remove-wins frontier over acceptances and revokes. An acceptance before or
concurrent with a revoke remains historical but is not active. A post-revoke acceptance is valid
only through a new grant whose lineage descends from every maximal revoke. Account actions cite
either the exact creator root or an acceptance for the same account and author. A historically
authorized action before a revoke remains projected; concurrent/post-revoke activity fails closed.

Local account selection independently requires the local installation root and an exact creator or
active-acceptance membership authority. Concurrent selections remain an explicit multivalue
register. A later selection resolves them only by descending from every maximum.

## Executable scenario map

`crates/hq-testkit/tests/authority_reduction.rs` maps and executes `AUTH-001` through `AUTH-022`.
The cases group the matrix by invariant: local signer and reserved mailbox; typed and directional
capability authority; observed/concurrent/post-revoke actions; partial and complete regrant;
remove-wins peer routing; exact device acceptance; missing/changed grants; concurrent revoke;
post-revoke reacceptance; wrong-account and arbitrary-selection attacks; conflicting account roots;
and revoked-source activity. The mailbox race fixture is reduced across every one of its 5,040
arrival permutations and compared as a complete normalized report.
