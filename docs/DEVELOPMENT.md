# Development

## Prerequisites

- Rust stable with Cargo.
- Docker for reproducible Linux cross-builds.
- `zip` for packaging examples.
- Android Debug Bridge for deploying to a speaker over ADB.

## Local development

```sh
cargo fmt
cargo check
cargo run -- --listen 127.0.0.1:8099 --data-dir ./data
```

The runtime persists settings and packages below `--data-dir`. Delete only a disposable development data directory when a clean state is required.

## Architecture

`App` owns shared settings, a device profile, LED controller and package manager. Axum serves the Web UI and JSON endpoints. Each Lua plugin runs in a blocking task with restricted Lua standard libraries. The runtime grants API tables only when the package requests the matching permission.

Plugin execution is asynchronous. A running plugin has an atomic cancellation token. `POST /api/plugins/:id/stop` signals the token and turns LEDs off. Lua code should yield frequently with `alice.sleep`.

## Device profiles

At startup YAFF inspects device-tree files, OS files, sysfs and available commands. The profile is available from `GET /api/device`.

Supported LED profiles:

| Profile | LED path | Count |
| --- | --- | --- |
| `mini2` | `mini2:{red,green,blue}:ringN` | 6 |
| `micro` | `micro:{red,green,blue}:ringN` | 3 |
| `sysfs-ring` | `{red,green,blue}:ringN` | configured |
| `custom` | `path_template` | configured |

For `custom`, the template supports `{root}`, `{channel}` and `{index}`. A configured, valid RGB ring adds `led.ring.rgb`, `led.ring.<count>` and `led.pattern.v1` to the profile at startup.

## Build targets

Station Lite detected during development uses AArch64 Linux. Build it reproducibly with:

```sh
docker run --rm -v "$PWD":/home/rust/src -w /home/rust/src messense/rust-musl-cross:aarch64-musl cargo build --release
```

The result is `target/aarch64-unknown-linux-musl/release/yaff`.

For armhf, use an armv7 musl cross-image/toolchain and produce an `armv7-unknown-linux-musleabihf` binary.

## Validation checklist

1. Run formatting and `cargo check`.
2. Start against a temporary data directory.
3. Install an example `.yap` package.
4. Confirm `GET /api/plugins` marks it compatible.
5. Start it and confirm `running: true`.
6. Stop it and confirm the hardware reaches its safe state.
