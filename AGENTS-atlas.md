# Atlas Codex Fork Notes

This branch is intended to stay close to upstream Codex. Keep local Atlas
workflow notes here instead of mixing them into upstream-facing docs.

## OVM branch snapshot flow

Use OVM dev installs as copied release-build snapshots, not live links and never
debug builds. Each meaningful local Codex branch build should get its own dev
label so Atlas can switch between known builds without a later `cargo build`
silently changing the binary under an existing OVM version.

From the repository root:

```bash
cd codex-rs
cargo build -p codex-cli --bin codex --release
LABEL="client-detach-resume-$(git rev-parse --short HEAD)"
ovm install codex dev --dev "$LABEL" --binary target/release/codex
ovm use codex "dev:$LABEL"
```

After OVM has copied the binary, Cargo build output is disposable:

```bash
rm -rf target/release target/debug
```

Check the active binary:

```bash
ovm current codex
ovm which codex
```

Notes:

- Do not pass `--link` for Atlas branch snapshots. OVM should copy the binary.
- Do not run or install `target/debug/codex` for Atlas testing. Debug builds
  can leave tens of GB in `target/debug` and are easy to accidentally snapshot.
- Use `target/release/codex` as the OVM input, then delete Cargo `target/`
  output when storage matters. The copied OVM snapshot remains usable.
- Use a new `--dev <label>` for each build you want to keep selectable.
- Include a short commit SHA, date, or build number in the label when useful.
- Re-run `cargo build -p codex-cli --bin codex --release` before installing a
  new snapshot.
- The regular `codex` shell function may point at OVM's active binary via
  `$HOME/.ovm/bin/codex`; verify with `ovm which codex`.
