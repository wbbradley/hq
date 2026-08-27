//! Exhaustive small arrival schedules and shrink-friendly command sequences.

/// Returns every arrival permutation for a deliberately small fixture.
pub fn arrival_permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
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

/// Ordered, shrink-friendly state-machine input sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateMachineSequence<T> {
    items: Vec<T>,
}

impl<T> StateMachineSequence<T> {
    /// Collects an explicit deterministic sequence.
    pub fn new(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    /// Borrows the first `length` inputs, clamped to the sequence length.
    pub fn prefix(&self, length: usize) -> &[T] {
        &self.items[..length.min(self.items.len())]
    }

    /// Borrows all inputs.
    pub fn items(&self) -> &[T] {
        &self.items
    }
}

impl<T: Clone> StateMachineSequence<T> {
    /// Returns a shrink candidate with one indexed input removed.
    #[must_use]
    pub fn without(&self, index: usize) -> Self {
        Self {
            items: self
                .items
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index)
                .map(|(_, item)| item.clone())
                .collect(),
        }
    }
}
