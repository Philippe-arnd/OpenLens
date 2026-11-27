## Product analytics (OpenLens)

### Keep the feature map current

- The app's features, success events and roles are declared in `openlens.features.yaml`.
- When you add, rename or remove a user-facing feature, update that file in the same change:
  its entry points (its `entry` routes and the `data-feature` attribute on its main control),
  its `paths` (globs of its code from the repository root, like `src/export/**`) and its
  `success` event. Add `data-feature="<key>"` to the feature's main control.
- When you move, add or delete code, keep the `paths` of the features it belongs to current:
  each release's changed files are matched against them to say which features it touched.
- When you add or rename a user role, map its raw value under `roles:`.
- Run `openlens-cli features lint --src .` before committing, then `openlens-cli features push` on deploy.
- The MCP tool `suggested_features` lists pages and `data-feature` values used in production that
  no feature claims, each with a proposed entry; `set_feature` adds or updates one entry. Write the
  map it returns to `openlens.features.yaml` and commit it: the file stays the source of truth.

### Keep releases measured

- The deploy step runs `openlens-cli release` (or the `openlens-release` GitHub Action) after the
  build, with the same release name the SDK sends. Do not remove it.
- After each deploy, read `release_report` through the OpenLens MCP server. When the verdict is
  `regressed`, read its evidence, fix the cause or roll back, and say so in the change that follows.
  Do not ship further features on top of a regressed release.
- Remove the flags that the MCP tool `stale_flags` reports in `ready_to_remove`: the flag in OpenLens
  (`openlens-cli flags delete <key>`) and its `isEnabled` branch in the code, keeping the path users
  now get (on for everyone) or dropping it (off).
- `openlens-cli verify` lists the release layers that are active; keep them all active.

### Privacy

- Never send names, emails or content to OpenLens: ids, roles, plans and event names only.
