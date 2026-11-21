# Wiring OpenLens into an app (agent guide)

A step-by-step guide that a coding agent follows to plug OpenLens into a web app with as little human work as possible. Status: draft, the tooling arrives in M2.

1. Run `openlens-cli wire --dry-run` at the app root. It detects the stack and prints an install plan: dependency or script tag, the init call, the `identify()` insertion point and feature naming hints.
2. Show the plan to the user and confirm the **role source**: which field on the logged-in user holds the role (for example `user.role`, `membership.kind`). If none is found, ask; do not guess.
3. Run `openlens-cli wire` to apply the plan as a single commit on a branch.
4. Run `openlens-cli verify`. It sends a test event and confirms it arrived with a role.
5. Open a PR that lists the edits, what was verified and what was not.

Never put the project API key anywhere other than the app's existing configuration or env mechanism.

## Keeping the feature map current

`wire` drafts `openlens.features.yaml` from the app's routes and main actions, and adds this rule to the app's own `AGENTS.md` (or `CLAUDE.md`):

> When you add, rename or remove a user-facing feature, update `openlens.features.yaml` (key, entry points, success event) in the same change, and run `openlens-cli features lint`.

For each feature, the most important field is its **success event**: the moment the user got value from it. Stick drivers and churn points are computed from it. See `docs/schema/features.example.yaml`.
