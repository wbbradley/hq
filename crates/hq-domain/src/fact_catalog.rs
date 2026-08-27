//! Stable semantic fact catalog identities and classes.

/// Protocol namespace that owns a semantic record.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProtocolClass {
    /// Immutable canonical knowledge reduced into product state.
    Canonical,
    /// Signed control-plane knowledge that cannot mutate project state directly.
    RemoteControl,
}

/// Retention contract for accepted semantic records.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RetentionClass {
    /// Canonical fact is retained permanently.
    CanonicalPermanent,
    /// Canonical fact is permanent while its materialized view may compact.
    CanonicalCompactedView,
    /// Remote-control record is retained permanently.
    ControlPermanent,
}

macro_rules! fact_catalog {
    ($(($kind:ident, $id:literal, $protocol:ident, $retention:ident)),+ $(,)?) => {
        /// Exhaustive semantic fact family identifier.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum FactKind {
            $(
                #[doc = concat!("Semantic fact family `", stringify!($kind), "`.")]
                $kind
            ),+
        }

        impl FactKind {
            /// Every catalog family in stable catalog order.
            pub const ALL: [Self; 48] = [$(Self::$kind),+];

            /// Returns the stable normative catalog ID.
            pub const fn catalog_id(self) -> &'static str {
                match self { $(Self::$kind => $id),+ }
            }

            /// Returns the stable semantic family name.
            pub const fn name(self) -> &'static str {
                match self { $(Self::$kind => stringify!($kind)),+ }
            }

            /// Returns the owning protocol namespace.
            pub const fn protocol_class(self) -> ProtocolClass {
                match self { $(Self::$kind => ProtocolClass::$protocol),+ }
            }

            /// Returns the retention contract.
            pub const fn retention_class(self) -> RetentionClass {
                match self { $(Self::$kind => RetentionClass::$retention),+ }
            }
        }
    };
}

fact_catalog!(
    (
        InstallationDeclared,
        "FCT-001",
        Canonical,
        CanonicalPermanent
    ),
    (MailboxCreated, "FCT-002", Canonical, CanonicalPermanent),
    (
        MailboxSessionBound,
        "FCT-003",
        Canonical,
        CanonicalPermanent
    ),
    (
        MailboxContextRecorded,
        "FCT-004",
        Canonical,
        CanonicalPermanent
    ),
    (PeerRouteSet, "FCT-005", Canonical, CanonicalPermanent),
    (PeerRouteBlocked, "FCT-006", Canonical, CanonicalPermanent),
    (
        MailboxAccessGranted,
        "FCT-007",
        Canonical,
        CanonicalPermanent
    ),
    (
        MailboxAccessRevoked,
        "FCT-008",
        Canonical,
        CanonicalPermanent
    ),
    (
        MailboxActionObserved,
        "FCT-009",
        Canonical,
        CanonicalPermanent
    ),
    (
        HumanAccountCreated,
        "FCT-010",
        Canonical,
        CanonicalPermanent
    ),
    (
        HumanAccountSelected,
        "FCT-011",
        Canonical,
        CanonicalPermanent
    ),
    (HumanDeviceGranted, "FCT-012", Canonical, CanonicalPermanent),
    (
        HumanDeviceAccepted,
        "FCT-013",
        Canonical,
        CanonicalPermanent
    ),
    (HumanDeviceRevoked, "FCT-014", Canonical, CanonicalPermanent),
    (QuestionAsked, "FCT-015", Canonical, CanonicalPermanent),
    (
        AsynchronousMessageSent,
        "FCT-016",
        Canonical,
        CanonicalPermanent
    ),
    (AnswerGiven, "FCT-017", Canonical, CanonicalPermanent),
    (ThreadCancelled, "FCT-018", Canonical, CanonicalPermanent),
    (MessageArchived, "FCT-019", Canonical, CanonicalPermanent),
    (MessageRestored, "FCT-020", Canonical, CanonicalPermanent),
    (MessageRejected, "FCT-021", Canonical, CanonicalPermanent),
    (
        HarnessActivityRecorded,
        "FCT-022",
        Canonical,
        CanonicalCompactedView
    ),
    (AgentNameClaimed, "FCT-023", Canonical, CanonicalPermanent),
    (AgentRetired, "FCT-024", Canonical, CanonicalPermanent),
    (
        ProviderSessionSelected,
        "FCT-025",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProviderSessionRenamed,
        "FCT-026",
        Canonical,
        CanonicalPermanent
    ),
    (ProjectCreated, "FCT-027", Canonical, CanonicalPermanent),
    (ProjectOpened, "FCT-028", Canonical, CanonicalPermanent),
    (
        ProjectClosingStarted,
        "FCT-029",
        Canonical,
        CanonicalPermanent
    ),
    (ProjectClosed, "FCT-030", Canonical, CanonicalPermanent),
    (ProjectArchived, "FCT-031", Canonical, CanonicalPermanent),
    (ProjectUnarchived, "FCT-032", Canonical, CanonicalPermanent),
    (
        ProjectMetadataUpdated,
        "FCT-033",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectResourceAdded,
        "FCT-034",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectResourceRemoved,
        "FCT-035",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectResourceReplaced,
        "FCT-036",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectPrimaryResourceChanged,
        "FCT-037",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectResourceHealthObserved,
        "FCT-038",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectAssignmentConfiguring,
        "FCT-039",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectAssignmentRunnable,
        "FCT-040",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectAssignmentBlocked,
        "FCT-041",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectAssignmentEnded,
        "FCT-042",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectInputAccepted,
        "FCT-043",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectInputDispatched,
        "FCT-044",
        Canonical,
        CanonicalPermanent
    ),
    (
        ProjectOutputRecorded,
        "FCT-045",
        Canonical,
        CanonicalPermanent
    ),
    (
        RemoteProjectCommandRequested,
        "FCT-046",
        RemoteControl,
        ControlPermanent
    ),
    (
        RemoteProjectCommandReceipt,
        "FCT-047",
        RemoteControl,
        ControlPermanent
    ),
    (
        RemoteProjectCommandOutcome,
        "FCT-048",
        RemoteControl,
        ControlPermanent
    ),
);
