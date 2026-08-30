Always try to use good software practices. In particular: encapsulation, DRY, test-first, and
decoupling.

Use Conventional Commits.

We are in a pre-release phase with not backwards compatibility requirements. Do not spend resources
on backwards compatibility with prior builds yet.

`scripts/hq-bootstrap` is a destructive developer helper for repeatedly testing the fresh customer
onboarding journey: it rebuilds HQ, resets local state, and creates a new identity and human account.
