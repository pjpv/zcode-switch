# Native login and saved claim context

Account switching restores the full saved credential JSON, including unknown
fields. Native ZCode owns its installation telemetry-state.json and RUM stores.
Switch must not overwrite deviceMid, _arms_uid, or _arms_session when selecting,
saving or restoring an account. Missing native device state is initialized by
ZCode, never by creating or rotating a replacement ID in Switch.

Claim eligibility has a separate saved-login context. On 2026-10-01, a limited
read-only comparison of two expired accounts returned empty eligibility with the
current installation MID and an available plan with each existing saved MID.
Balance queries were empty in both cases. Replacing the saved claim context with
the current installation MID therefore broke renewal eligibility for those
snapshots; an empty balance was the actual expired-plan result, not a rendering
failure.

Claim preview, activation and claim submission use the MID recorded with that
login (the historical virtual_device_mid field). It is read as-is and never
written to native device state. Snapshots without a recorded MID use the actual
native MID. Native capture and OAuth persistence record the MID actually used,
without uniqueness checks or random generation; a completed new OAuth login
updates the context along with its credentials. Encrypted account bundles retain
this optional context as claimDeviceMid while accepting older version-2 bundles.
Legacy virtual_arms_uid metadata is retained for compatibility and not applied.

Updating an existing snapshot archives refreshed tokens/configuration and retains
its existing claim context. It does not infer a new login from a token change.

Native OAuth and quota/model-side state use the current installation MID.
An explicitly expired target JWT is rejected before closing ZCode or writing
login files. An already active account first synchronizes native refreshed
credentials back to its snapshot. An empty successful balance is displayed as
no active quota, and an eligibility query failure is displayed separately.

When automatic claiming is enabled, restoring the persisted setting starts a
round after one second. Busy operations retain the pending round and retry the
scheduling check after five seconds. Rounds run serially, with the next periodic
round ten minutes after completion. Closing the toggle cancels future work
before its persistence request completes. Qualification refresh can request an
earlier round without another toggle.

Each attempt registers its result waiter before opening verification and carries
one claimId through the window, submission, cancellation and result. Late events
cannot complete or cancel a different attempt. Automatic verification that
requires interaction stops before notifying the main window, so a late SDK
success cannot race its cancellation. Manual interactive verification remains
available. SDK setup failures terminate the waiter separately from challenges.

Verification:

```powershell
npm run tauri -- build --no-bundle
npm run test:login -- src-tauri/target/release/zcode-switch.exe
npm run test:claims
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

Synthetic CLI checks explicitly isolate home/data directories and disable
process control and upstream probes through existing sandbox mode. They verify
forced restore, already-active synchronization, missing native state, expired
snapshots, opaque credential preservation and automatic account preservation.
Rust checks additionally cover distinct saved/native claim contexts and bundle
round trips. Live read-only eligibility must be checked on the final executable.
These checks do not establish CAPTCHA submission, actual grants, a running
client's hot reload, or elimination of service-side 3012. Real renewal and a
same-account/model/network request remain separate acceptance steps.
