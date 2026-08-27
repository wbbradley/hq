//! Executable laws for the pure complete-batch causal reducer.

use std::{collections::BTreeSet, error::Error};

use hq_domain::{
    AuthorityRole, BoundedSet, CausalReferences, Fact, FactId, FactScope, InstallationAddress,
    InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxId, MailboxKind,
    SemanticPayload, ShortText, SigningPublicKey, Timestamp,
};
use hq_reducer::{
    ConflictReason, DecisionReason, DecisionStatus, DomainDecision, DomainReducer, FactSet,
    PresentationEntry, PresentationError, PresentationFamily, PresentationKey,
    ProjectionContribution, ReduceError, ReductionContext, canonical_presentation_order,
    reduce_complete,
};
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TestReason {
    Rejected,
    Conflict,
    Invalid,
    Unsupported,
}

#[derive(Clone, Debug, Default)]
struct TestReducer {
    rejected: BTreeSet<FactId>,
    aggregate_all: bool,
    project_tip: Option<FactId>,
}

impl DomainReducer for TestReducer {
    type AggregateKey = u8;
    type ProjectionKey = u8;
    type ProjectionValue = FactId;
    type Reason = TestReason;

    fn classify(
        &self,
        fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        if self.rejected.contains(&fact.id()) {
            DomainDecision::Unauthorized {
                reason: TestReason::Rejected,
                failed_authorities: BTreeSet::new(),
            }
        } else {
            DomainDecision::Projected
        }
    }

    fn aggregate_keys(
        &self,
        _fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<Self::AggregateKey> {
        if self.aggregate_all {
            vec![0]
        } else {
            Vec::new()
        }
    }

    fn projections(
        &self,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ProjectionContribution<Self::ProjectionKey, Self::ProjectionValue>> {
        self.project_tip
            .map(|fact_id| ProjectionContribution::new(0, fact_id, [fact_id]))
            .into_iter()
            .collect()
    }
}

fn id(value: u8) -> FactId {
    let mut bytes = [0; 32];
    bytes[31] = value;
    FactId::from_bytes(bytes)
}

fn fact(value: u8, parents: &[u8], authored_at: i64) -> Result<Fact, Box<dyn Error>> {
    let installation_id = InstallationId::from_bytes([7; 32]);
    let author = InstallationAddress::new(installation_id, SigningPublicKey::from_bytes([8; 32]));
    Ok(Fact::new(
        id(value),
        author,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(installation_id),
        CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
            BoundedSet::new(parents.iter().copied().map(id))?,
            [],
        )?,
        SemanticPayload::MailboxCreated {
            mailbox_id: MailboxId::from_bytes([value; 32]),
            kind: MailboxKind::Agent,
            label: Some(ShortText::new(format!("fact-{value}"))?),
        },
    )?)
}

fn generated_dags(node_count: u8) -> Vec<Vec<Vec<u8>>> {
    let possible_edges = (0..node_count)
        .flat_map(|child| (0..child).map(move |parent| (parent, child)))
        .collect::<Vec<_>>();
    (0..(1_usize << possible_edges.len()))
        .map(|mask| {
            let mut parents = vec![Vec::new(); usize::from(node_count)];
            for (edge, (parent, child)) in possible_edges.iter().copied().enumerate() {
                if mask & (1 << edge) != 0 {
                    parents[usize::from(child)].push(parent);
                }
            }
            parents
        })
        .collect()
}

fn arrival_permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    fn visit<T: Clone>(remaining: &mut Vec<T>, prefix: &mut Vec<T>, output: &mut Vec<Vec<T>>) {
        if remaining.is_empty() {
            output.push(prefix.clone());
            return;
        }
        for index in 0..remaining.len() {
            let item = remaining.remove(index);
            prefix.push(item.clone());
            visit(remaining, prefix, output);
            if let Some(restored) = prefix.pop() {
                remaining.insert(index, restored);
            }
        }
    }

    let mut output = Vec::new();
    visit(&mut items.to_vec(), &mut Vec::new(), &mut output);
    output
}

#[test]
fn law_merge_set_union_is_a_semilattice() -> Result<(), Box<dyn Error>> {
    let a = fact(1, &[], 1)?;
    let b = fact(2, &[1], 2)?;
    let c = fact(3, &[1], 3)?;
    let empty = FactSet::default();
    let left = FactSet::from_facts([a.clone(), b.clone()]);
    let right = FactSet::from_facts([b.clone(), c.clone()]);
    let third = FactSet::from_facts([a, c]);

    assert_eq!(left.merge(&right), right.merge(&left));
    assert_eq!(left.merge(&left), left);
    assert_eq!(left.merge(&empty), left);
    assert_eq!(
        left.merge(&right).merge(&third),
        left.merge(&right.merge(&third))
    );
    let collision = FactSet::from_facts([fact(9, &[], 1)?, fact(9, &[], 2)?]);
    assert!(collision.is_collision(id(9)));
    assert_eq!(collision.merge(&left), left.merge(&collision));
    assert_eq!(collision.merge(&collision), collision);
    Ok(())
}

#[test]
fn law_input_invariance_covers_permutations_and_duplicates() -> Result<(), Box<dyn Error>> {
    let facts = vec![fact(1, &[], 30)?, fact(2, &[1], 10)?, fact(3, &[1], 20)?];
    let expected = reduce_complete(facts.clone(), &TestReducer::default())?;

    for mut arrival in arrival_permutations(&facts) {
        arrival.push(facts[1].clone());
        assert_eq!(reduce_complete(arrival, &TestReducer::default())?, expected);
    }
    Ok(())
}

#[test]
fn graph_tracks_both_directions_and_structural_reachability() -> Result<(), Box<dyn Error>> {
    let report = reduce_complete(
        [fact(1, &[], 30)?, fact(2, &[1], 20)?, fact(3, &[2], 10)?],
        &TestReducer::default(),
    )?;

    assert_eq!(report.graph().parents(id(3)), &BTreeSet::from([id(2)]));
    assert_eq!(report.graph().children(id(1)), &BTreeSet::from([id(2)]));
    assert!(report.graph().structurally_reaches(id(1), id(3)));
    assert!(!report.graph().structurally_reaches(id(3), id(1)));
    assert_eq!(
        report.graph().reverse_dependant_closure([id(1)]),
        BTreeSet::from([id(1), id(2), id(3)])
    );
    assert!(report.usably_reaches(id(1), id(3)));
    assert_eq!(report.dependency_order(), &[id(1), id(2), id(3)]);
    Ok(())
}

#[test]
fn law_deferred_readiness_separates_missing_and_unusable_parents() -> Result<(), Box<dyn Error>> {
    let child = fact(2, &[1], 2)?;
    let incomplete = reduce_complete([child.clone()], &TestReducer::default())?;
    let decision = &incomplete.decisions()[&id(2)];
    assert_eq!(decision.status(), DecisionStatus::Unresolved);
    assert_eq!(decision.missing_dependencies(), &BTreeSet::from([id(1)]));
    assert!(decision.unusable_dependencies().is_empty());

    let complete = reduce_complete([fact(1, &[], 1)?, child.clone()], &TestReducer::default())?;
    assert_eq!(
        complete.decisions()[&id(2)].status(),
        DecisionStatus::Projected
    );

    let reducer = TestReducer {
        rejected: BTreeSet::from([id(1)]),
        ..TestReducer::default()
    };
    let rejected = reduce_complete([fact(1, &[], 1)?, child], &reducer)?;
    assert_eq!(
        rejected.decisions()[&id(2)].status(),
        DecisionStatus::Unresolved
    );
    assert_eq!(
        rejected.decisions()[&id(2)].unusable_dependencies()[&id(1)],
        DecisionStatus::Unauthorized
    );
    Ok(())
}

#[test]
fn law_causal_dominance_excludes_an_unusable_bridge() -> Result<(), Box<dyn Error>> {
    let reducer = TestReducer {
        rejected: BTreeSet::from([id(1)]),
        aggregate_all: true,
        ..TestReducer::default()
    };
    let report = reduce_complete(
        [fact(1, &[], 1)?, fact(2, &[1], 2)?, fact(3, &[], 3)?],
        &reducer,
    )?;

    assert!(report.graph().structurally_reaches(id(1), id(2)));
    assert!(!report.usably_reaches(id(1), id(2)));
    assert_eq!(report.frontiers()[&0], BTreeSet::from([id(3)]));
    Ok(())
}

#[test]
fn collisions_and_exact_cycle_members_fail_closed() -> Result<(), Box<dyn Error>> {
    let collision_a = fact(1, &[], 1)?;
    let collision_b = fact(1, &[], 2)?;
    let collision_report = reduce_complete(
        [collision_a, collision_b, fact(2, &[1], 3)?],
        &TestReducer::default(),
    )?;
    assert_eq!(
        collision_report.decisions()[&id(1)].status(),
        DecisionStatus::Conflicted
    );
    assert_eq!(
        collision_report.decisions()[&id(1)].reason(),
        Some(&DecisionReason::IdentityCollision)
    );
    assert_eq!(
        collision_report.decisions()[&id(2)].status(),
        DecisionStatus::Unresolved
    );

    let cycle_report = reduce_complete(
        [fact(1, &[2], 1)?, fact(2, &[1], 2)?, fact(3, &[2], 3)?],
        &TestReducer::default(),
    )?;
    assert_eq!(
        cycle_report.decisions()[&id(1)].status(),
        DecisionStatus::Invalid
    );
    assert_eq!(
        cycle_report.decisions()[&id(2)].status(),
        DecisionStatus::Invalid
    );
    assert_eq!(
        cycle_report.decisions()[&id(3)].status(),
        DecisionStatus::Unresolved
    );
    assert_eq!(
        cycle_report.decisions()[&id(1)].reason(),
        Some(&DecisionReason::CausalCycle)
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct DecisionReducer;

impl DomainReducer for DecisionReducer {
    type AggregateKey = u8;
    type ProjectionKey = u8;
    type ProjectionValue = u8;
    type Reason = TestReason;

    fn classify(
        &self,
        fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        match fact.id().as_bytes()[31] {
            1 => DomainDecision::Unauthorized {
                reason: TestReason::Rejected,
                failed_authorities: BTreeSet::from([AuthorityRole::Grant]),
            },
            2 | 3 => DomainDecision::Conflicted {
                reason: TestReason::Conflict,
                participants: BTreeSet::from([id(2), id(3)]),
            },
            4 => DomainDecision::Invalid {
                reason: TestReason::Invalid,
            },
            5 => DomainDecision::Unsupported {
                reason: TestReason::Unsupported,
            },
            _ => DomainDecision::Projected,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct OscillatingReducer;

impl DomainReducer for OscillatingReducer {
    type AggregateKey = u8;
    type ProjectionKey = u8;
    type ProjectionValue = u8;
    type Reason = TestReason;

    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        if context.is_projected(fact.id()) {
            DomainDecision::Invalid {
                reason: TestReason::Invalid,
            }
        } else {
            DomainDecision::Projected
        }
    }
}

#[test]
fn an_oscillating_domain_stage_fails_explicitly() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        reduce_complete([fact(1, &[], 1)?], &OscillatingReducer),
        Err(ReduceError::NonConvergentDomainDecisions)
    );
    Ok(())
}

#[test]
fn report_normalizes_every_domain_decision_and_conflict() -> Result<(), Box<dyn Error>> {
    let report = reduce_complete(
        (1..=6)
            .map(|value| fact(value, &[], i64::from(value)))
            .collect::<Result<Vec<_>, _>>()?,
        &DecisionReducer,
    )?;

    assert_eq!(
        report.decisions()[&id(1)].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&id(1)].failed_authorities(),
        &BTreeSet::from([AuthorityRole::Grant])
    );
    assert_eq!(
        report.decisions()[&id(2)].status(),
        DecisionStatus::Conflicted
    );
    assert_eq!(report.decisions()[&id(4)].status(), DecisionStatus::Invalid);
    assert_eq!(
        report.decisions()[&id(5)].status(),
        DecisionStatus::Unsupported
    );
    assert_eq!(
        report.decisions()[&id(6)].status(),
        DecisionStatus::Projected
    );
    assert!(report.conflicts().iter().any(|conflict| {
        conflict.reason() == &ConflictReason::Domain(TestReason::Conflict)
            && conflict.participants() == &BTreeSet::from([id(2), id(3)])
    }));
    Ok(())
}

#[test]
fn generated_dags_have_exact_frontiers_and_parent_first_order() -> Result<(), Box<dyn Error>> {
    for dag in generated_dags(4) {
        let facts = dag
            .iter()
            .enumerate()
            .map(|(node, parents)| {
                fact(
                    u8::try_from(node + 1)?,
                    &parents.iter().map(|p| p + 1).collect::<Vec<_>>(),
                    100 - i64::try_from(node)?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let reducer = TestReducer {
            aggregate_all: true,
            ..TestReducer::default()
        };
        let report = reduce_complete(facts, &reducer)?;
        let order = report.dependency_order();
        let positions = order
            .iter()
            .copied()
            .enumerate()
            .map(|(position, fact_id)| (fact_id, position))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (child, parents) in dag.iter().enumerate() {
            for parent in parents {
                assert!(positions[&id(parent + 1)] < positions[&id(u8::try_from(child + 1)?)]);
            }
        }
        let expected = (0..4_u8)
            .filter(|candidate| {
                !dag.iter()
                    .skip(usize::from(*candidate + 1))
                    .any(|parents| parents.contains(candidate))
            })
            .map(|candidate| id(candidate + 1))
            .collect::<BTreeSet<_>>();
        assert_eq!(report.frontiers()[&0], expected);
    }
    Ok(())
}

#[test]
fn projection_support_expands_through_usable_ancestors() -> Result<(), Box<dyn Error>> {
    let reducer = TestReducer {
        project_tip: Some(id(3)),
        ..TestReducer::default()
    };
    let report = reduce_complete(
        [fact(1, &[], 1)?, fact(2, &[1], 2)?, fact(3, &[2], 3)?],
        &reducer,
    )?;

    assert_eq!(report.projections()[&0], id(3));
    assert_eq!(report.support()[&0], BTreeSet::from([id(1), id(2), id(3)]));
    Ok(())
}

#[test]
fn presentation_comparator_preserves_causality_before_clock_order() -> Result<(), Box<dyn Error>> {
    let facts = FactSet::from_facts([
        fact(1, &[], 100)?,
        fact(2, &[1], -100)?,
        fact(3, &[], 0)?,
        fact(4, &[], 0)?,
    ]);
    let entries = [
        PresentationEntry::new(
            id(1),
            PresentationKey::minimal(
                Timestamp::from_unix_millis(100),
                PresentationFamily::Message,
            ),
        ),
        PresentationEntry::new(
            id(2),
            PresentationKey::minimal(
                Timestamp::from_unix_millis(-100),
                PresentationFamily::Message,
            ),
        ),
        PresentationEntry::new(
            id(3),
            PresentationKey::minimal(Timestamp::from_unix_millis(0), PresentationFamily::Activity),
        ),
        PresentationEntry::new(
            id(4),
            PresentationKey::minimal(Timestamp::from_unix_millis(0), PresentationFamily::Message),
        ),
    ];

    assert_eq!(
        canonical_presentation_order(&facts.graph(), entries)?,
        vec![id(4), id(3), id(1), id(2)]
    );
    Ok(())
}

#[test]
fn presentation_comparator_rejects_a_selected_cycle() -> Result<(), Box<dyn Error>> {
    let facts = FactSet::from_facts([fact(1, &[2], 1)?, fact(2, &[1], 2)?]);
    let entries = [
        PresentationEntry::new(
            id(1),
            PresentationKey::minimal(Timestamp::from_unix_millis(1), PresentationFamily::Message),
        ),
        PresentationEntry::new(
            id(2),
            PresentationKey::minimal(Timestamp::from_unix_millis(2), PresentationFamily::Message),
        ),
    ];

    assert_eq!(
        canonical_presentation_order(&facts.graph(), entries),
        Err(PresentationError::CausalCycle)
    );
    Ok(())
}
