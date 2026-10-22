# YAFF

YAFF is a local Rust runtime for Yandex smart speakers. It provides a Web UI, installs `.yap` packages, executes Lua plugins and exposes LED, audio, Bluetooth, display and system APIs.

```sh
cargo check
cargo run -- --data-dir ./data
```

Open `http://<speaker-ip>:8099`.

Documentation:

- [Development](docs/DEVELOPMENT.md)
- [Plugin API](docs/PLUGIN_API.md)
- [Deployment](docs/DEPLOYMENT.md)
- [Agent instructions](AGENTS.md)

Examples live in [`examples/`](examples/).
