//! Explicit pure-model invalidation performance regression budget.

#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use hq_tui::{
    UiEffect, UiEvent, UiHumanState, UiModel, UiRow, UiRowKind, UiRowState, UiSize, UiSnapshot,
    update,
};

const REPRESENTATIVE_ROW_COUNT: usize = 10_000;

fn budget() -> Duration {
    let milliseconds = std::env::var("HQ_QUALIFICATION_INVALIDATION_REDRAW_MAX_MILLISECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(100);
    Duration::from_millis(milliseconds)
}

#[test]
fn invalidation_of_a_large_ready_model_requests_redraw_within_the_declared_budget() {
    let started = update(
        UiModel::new(UiSize {
            width: 120,
            height: 40,
        }),
        UiEvent::Started,
    )
    .expect("model starts");
    let snapshot_id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("startup requests a snapshot");
    let snapshot = UiSnapshot {
        revision: 1,
        human_state: UiHumanState::Ready,
        inbox_rows: (0..REPRESENTATIVE_ROW_COUNT)
            .map(|index| UiRow {
                id: format!("conversation-{index}"),
                title: format!("Conversation {index}"),
                detail: "ready".to_owned(),
                state: UiRowState::Open,
                kind: UiRowKind::Conversation,
            })
            .collect(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    };
    let ready = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot,
        },
    )
    .expect("large snapshot loads");

    let measured_at = Instant::now();
    let invalidated =
        update(ready.model, UiEvent::Invalidated { revision: 2 }).expect("invalidation applies");
    let elapsed = measured_at.elapsed();

    assert!(
        invalidated
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
    assert!(invalidated.effects.contains(&UiEffect::RequestRedraw));
    let maximum = budget();
    assert!(
        elapsed <= maximum,
        "invalidation-to-redraw took {elapsed:?}, exceeding {maximum:?}"
    );
}
