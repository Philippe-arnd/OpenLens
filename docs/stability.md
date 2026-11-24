# Stability

OpenLens is **beta** (0.x) until the end of milestone M6, when it is used on real apps (#42). This page lists what a beta release may change, what it may not, and how an upgrade goes. It is the policy that every **Breaking:** line of the [CHANGELOG](../CHANGELOG.md) refers to.

## What a 0.x release may change

A minor release (0.x.0) may change behaviour, and each change is listed under **Changed**. A change that asks the person who runs the server, or an app that sends events, to do something is a breaking change. It starts with **Breaking:** and links here. A patch release (0.x.y) never breaks anything.

## What stays compatible

- **Stored events.** A newer server reads the Parquet files written by an older one, with no migration. Columns are only added, and an older file simply has no value in the new ones (the release, environment and flag columns were added this way). A column is never renamed or retyped in place.
- **The feature map.** Fields are only added, and they are always optional, so a map that worked keeps working. The parser is strict (`deny_unknown_fields`), so an *older* server refuses a map with a new field. Upgrade the server before you push a map that uses one (D-019).
- **The SDKs.** A newer server accepts what an older SDK sends. `/capture`, `/r` and `/flags` keep accepting every field they accepted before.
- **The HTTP API.** `/api/...` is not versioned yet. It moves to `/api/v1` with #312, which becomes the contract. From then on, a field or an endpoint is removed only after it has been marked deprecated in the CHANGELOG for **at least one minor release**.
- **Settings.** An environment variable is never renamed without the old name working for at least one minor release (there are few of them: D-016).

## Upgrade path

1. Back up the data dir (the server also makes a copy before each upgrade).
2. Upgrade the server. It reads the old data as it is.
3. Then push a feature map that uses new fields, if any.
4. Then upgrade the SDKs, whenever it suits you: old ones keep working.

Downgrading is not supported once the newer server has written data. Restore the copy instead.

## After the beta

From 1.0.0 on, a breaking change waits for a major version, following [SemVer](https://semver.org/spec/v2.0.0.html).
