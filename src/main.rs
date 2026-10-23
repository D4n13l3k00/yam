use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use clap::Parser;
use mlua::{Lua, LuaOptions, StdLib};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex, task};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:8099")]
    listen: String,
    #[arg(long, default_value = "/opt/yam")]
    data_dir: PathBuf,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    led: LedSettings,
    #[serde(default)]
    allow_plugin_binaries: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedSettings {
    profile: String,
    #[serde(default = "default_led_count")]
    count: usize,
    #[serde(default = "default_led_root")]
    sysfs_root: String,
    #[serde(default)]
    path_template: Option<String>,
}
fn default_led_count() -> usize {
    6
}
fn default_led_root() -> String {
    "/sys/class/leds".into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            led: LedSettings {
                profile: "mini2".into(),
                count: 6,
                sysfs_root: default_led_root(),
                path_template: None,
            },
            allow_plugin_binaries: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    entry: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    settings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    compatibility: Compatibility,
    #[serde(default)]
    launch: Launch,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Launch {
    #[serde(default)]
    kind: LaunchKind,
    #[serde(default)]
    mode: LaunchMode,
    #[serde(default)]
    path: String,
    #[serde(default)]
    args: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum LaunchKind {
    #[default]
    Lua,
    Binary,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum LaunchMode {
    #[default]
    Oneshot,
    Service,
}
impl Launch {
    fn path<'a>(&'a self, manifest: &'a Manifest) -> &'a str {
        if self.path.is_empty() {
            &manifest.entry
        } else {
            &self.path
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Compatibility {
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceProfile {
    model: String,
    architecture: String,
    features: Vec<String>,
    source: String,
}
#[derive(Debug, Clone, Serialize)]
struct PluginInfo {
    manifest: Manifest,
    enabled: bool,
    running: bool,
    compatible: bool,
    incompatibility: Option<String>,
}
#[derive(Clone)]
struct App {
    root: PathBuf,
    settings: Arc<Mutex<Settings>>,
    device: Arc<Mutex<DeviceProfile>>,
    led: Arc<Led>,
    plugins: Arc<Mutex<PluginManager>>,
}
#[derive(Clone)]
struct Led {
    settings: Arc<Mutex<LedSettings>>,
}
#[derive(Deserialize)]
struct Frame {
    colors: Vec<String>,
    #[serde(default = "frame_delay")]
    delay_ms: u64,
}
fn frame_delay() -> u64 {
    100
}

impl Led {
    async fn draw(&self, colors: Vec<String>) -> Result<()> {
        let c = self.settings.lock().await.clone();
        if colors.len() != c.count {
            bail!("expected {} colors, got {}", c.count, colors.len())
        }
        for (i, hex) in colors.iter().enumerate() {
            let (r, g, b) = parse_color(hex)?;
            self.write(&c, "red", i, r).await?;
            self.write(&c, "green", i, g).await?;
            self.write(&c, "blue", i, b).await?;
        }
        Ok(())
    }
    async fn off(&self) -> Result<()> {
        let n = self.settings.lock().await.count;
        self.draw(vec!["#000000".into(); n]).await
    }
    async fn write(&self, c: &LedSettings, channel: &str, i: usize, value: u8) -> Result<()> {
        let path = match c.profile.as_str() {
            "mini2" => PathBuf::from(&c.sysfs_root)
                .join(format!("mini2:{channel}:ring{}/brightness", i + 1)),
            "micro" => PathBuf::from(&c.sysfs_root)
                .join(format!("micro:{channel}:ring{}/brightness", i + 1)),
            "sysfs-ring" => {
                PathBuf::from(&c.sysfs_root).join(format!("{channel}:ring{}/brightness", i + 1))
            }
            "custom" => PathBuf::from(
                c.path_template
                    .as_ref()
                    .ok_or_else(|| anyhow!("custom profile needs path_template"))?
                    .replace("{root}", &c.sysfs_root)
                    .replace("{channel}", channel)
                    .replace("{index}", &(i + 1).to_string()),
            ),
            p => bail!("unknown LED profile {p}"),
        };
        fs::write(&path, value.to_string()).with_context(|| format!("write {}", path.display()))
    }
    async fn play_led(&self, path: PathBuf) -> Result<()> {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read pattern {}", path.display()))?;
        let (looped, frames) = parse_pattern(&source)?;
        if frames.is_empty() {
            bail!("pattern has no frames")
        };
        loop {
            for f in &frames {
                self.draw(f.colors.clone()).await?;
                tokio::time::sleep(Duration::from_millis(f.delay_ms)).await;
            }
            if !looped {
                break;
            }
        }
        Ok(())
    }
}
fn parse_color(s: &str) -> Result<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        bail!("invalid RGB color {s}")
    };
    Ok((
        u8::from_str_radix(&h[0..2], 16)?,
        u8::from_str_radix(&h[2..4], 16)?,
        u8::from_str_radix(&h[4..6], 16)?,
    ))
}
fn parse_pattern(s: &str) -> Result<(bool, Vec<Frame>)> {
    let mut looped = false;
    let mut frames = vec![];
    for line in s
        .lines()
        .map(str::trim)
        .filter(|x| !x.is_empty() && !x.starts_with('#'))
    {
        if line.eq_ignore_ascii_case("loop") {
            looped = true;
            continue;
        }
        if line.eq_ignore_ascii_case("gammacorrection") {
            continue;
        };
        let p: Vec<_> = line.split_whitespace().collect();
        if p.len() < 2 {
            continue;
        };
        let delay = p.last().unwrap().parse().context("bad frame delay")?;
        frames.push(Frame {
            colors: p[..p.len() - 1].iter().map(|x| format!("#{x}")).collect(),
            delay_ms: delay,
        });
    }
    Ok((looped, frames))
}

struct PluginManager {
    root: PathBuf,
    enabled: HashSet<String>,
    running: HashMap<String, Arc<AtomicBool>>,
}
impl PluginManager {
    fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let enabled = fs::read_to_string(root.join("enabled.json"))
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        Ok(Self {
            root,
            enabled,
            running: HashMap::new(),
        })
    }
    fn save(&self) -> Result<()> {
        fs::write(
            self.root.join("enabled.json"),
            serde_json::to_vec_pretty(&self.enabled)?,
        )?;
        Ok(())
    }
    fn list(&self, device: &DeviceProfile) -> Result<Vec<PluginInfo>> {
        let mut out = vec![];
        for e in fs::read_dir(&self.root)? {
            let e = e?;
            if !e.file_type()?.is_dir() {
                continue;
            };
            let p = e.path().join("plugin.toml");
            if let Ok(x) = fs::read_to_string(p) {
                if let Ok(manifest) = toml::from_str::<Manifest>(&x) {
                    let reason = compatibility_error(&manifest, device);
                    out.push(PluginInfo {
                        enabled: self.enabled.contains(&manifest.id),
                        running: self.running.contains_key(&manifest.id),
                        compatible: reason.is_none(),
                        incompatibility: reason,
                        manifest,
                    })
                }
            }
        }
        Ok(out)
    }
    fn manifest(&self, id: &str) -> Result<Manifest> {
        validate_id(id)?;
        Ok(toml::from_str(&fs::read_to_string(
            self.root.join(id).join("plugin.toml"),
        )?)?)
    }
    fn install(&mut self, bytes: &[u8]) -> Result<Manifest> {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes))?;
        let manifest_text = {
            let mut f = z
                .by_name("plugin.toml")
                .context(".yap must contain plugin.toml")?;
            let mut x = String::new();
            f.read_to_string(&mut x)?;
            x
        };
        let m: Manifest = toml::from_str(&manifest_text)?;
        validate_id(&m.id)?;
        safe_relative(&m.entry)?;
        safe_relative(m.launch.path(&m))?;
        if m.launch.kind == LaunchKind::Binary && !m.permissions.iter().any(|p| p == "exec") {
            bail!("binary packages require the exec permission")
        }
        let stage = self.root.join(format!(".install-{}", Uuid::new_v4()));
        fs::create_dir(&stage)?;
        for i in 0..z.len() {
            let mut f = z.by_index(i)?;
            let name = f
                .enclosed_name()
                .ok_or_else(|| anyhow!("unsafe archive path"))?
                .to_owned();
            if f.is_dir() {
                fs::create_dir_all(stage.join(name))?
            } else {
                let dest = stage.join(name);
                if let Some(p) = dest.parent() {
                    fs::create_dir_all(p)?
                };
                let mode = f.unix_mode();
                let mut o = fs::File::create(&dest)?;
                std::io::copy(&mut f, &mut o)?;
                #[cfg(unix)]
                if let Some(mode) = mode {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&dest, fs::Permissions::from_mode(mode))?;
                }
            }
        }
        let target = self.root.join(&m.id);
        if target.exists() {
            fs::remove_dir_all(&target)?
        };
        fs::rename(stage, target)?;
        Ok(m)
    }
    fn remove(&mut self, id: &str) -> Result<()> {
        validate_id(id)?;
        let p = self.root.join(id);
        if !p.exists() {
            bail!("plugin not found")
        };
        fs::remove_dir_all(p)?;
        self.enabled.remove(id);
        self.save()
    }
}
fn compatibility_error(m: &Manifest, d: &DeviceProfile) -> Option<String> {
    if !m.compatibility.models.is_empty() && !m.compatibility.models.iter().any(|x| x == &d.model) {
        return Some(format!(
            "requires model [{}], detected {}",
            m.compatibility.models.join(", "),
            d.model
        ));
    };
    let present: HashSet<_> = d.features.iter().collect();
    let missing: Vec<_> = m
        .compatibility
        .features
        .iter()
        .filter(|x| !present.contains(x))
        .cloned()
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!("missing feature(s): {}", missing.join(", ")))
    }
}
fn detect_device(root: &Path, forced: Option<String>) -> DeviceProfile {
    if let Ok(raw) = fs::read_to_string(root.join("device.toml")) {
        if let Ok(p) = toml::from_str::<DeviceProfile>(&raw) {
            return p;
        }
    }
    let arch = std::env::consts::ARCH.to_string();
    let text = [
        "/proc/device-tree/model",
        "/proc/device-tree/compatible",
        "/etc/os-release",
    ]
    .iter()
    .filter_map(|p| fs::read_to_string(p).ok())
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    let mut model = forced.unwrap_or_else(|| {
        if Path::new("/sys/class/leds/micro:red:ring1/brightness").exists()
            || text.contains("yandexmicro")
        {
            "yandexmicro".into()
        } else if Path::new("/sys/class/leds/mini2:red:ring1/brightness").exists()
            || text.contains("yandexmini_2")
        {
            "yandexmini_2".into()
        } else if text.contains("yandexstation_2") {
            "yandexstation_2".into()
        } else if text.contains("yandexmini") {
            "yandexmini".into()
        } else if text.contains("yandexmicro") {
            "yandexmicro".into()
        } else {
            "unknown".into()
        }
    });
    let mut features = vec!["runtime.lua.v1".into(), "plugin.yap.v1".into()];
    if Path::new("/sys/class/leds/mini2:red:ring1/brightness").exists() {
        features.extend([
            "led.ring.rgb".into(),
            "led.ring.6".into(),
            "led.pattern.v1".into(),
        ])
    };
    if Path::new("/sys/class/leds").exists() && !features.iter().any(|x| x == "led.ring.rgb") {
        features.push("led.sysfs".into())
    };
    if command_exists("amixer") {
        features.push("audio.volume.alsa".into())
    }
    if command_exists("aplay") {
        features.push("audio.play.aplay".into())
    }
    if command_exists("bluetoothctl") {
        features.extend([
            "bluetooth.bluez".into(),
            "bluetooth.scan".into(),
            "bluetooth.pair".into(),
        ])
    }
    if Path::new("/sys/class/backlight").exists() {
        features.push("display.backlight".into())
    }
    if Path::new("/sys/class/input").exists() {
        features.push("input.evdev".into())
    }
    if arch.starts_with("arm") {
        features.push("arch.armhf".into())
    };
    if model == "unknown" {
        model = "unknown".into()
    };
    DeviceProfile {
        model,
        architecture: arch,
        features,
        source: "auto (override: data-dir/device.toml or --model)".into(),
    }
}
fn validate_id(s: &str) -> Result<()> {
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("invalid plugin id")
    };
    Ok(())
}
fn safe_relative(s: &str) -> Result<()> {
    let p = Path::new(s);
    if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("unsafe path")
    };
    Ok(())
}
fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(name).is_file()))
}
fn command_output(program: &str, args: &[String]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
fn valid_mac(mac: &str) -> bool {
    mac.len() == 17
        && mac.bytes().enumerate().all(|(i, b)| {
            if [2, 5, 8, 11, 14].contains(&i) {
                b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn configured_rgb_ring(settings: &LedSettings) -> bool {
    let sample = |channel: &str| -> Option<PathBuf> {
        match settings.profile.as_str() {
            "mini2" => Some(
                PathBuf::from(&settings.sysfs_root)
                    .join(format!("mini2:{channel}:ring1/brightness")),
            ),
            "micro" => Some(
                PathBuf::from(&settings.sysfs_root)
                    .join(format!("micro:{channel}:ring1/brightness")),
            ),
            "sysfs-ring" => Some(
                PathBuf::from(&settings.sysfs_root).join(format!("{channel}:ring1/brightness")),
            ),
            "custom" => settings.path_template.as_ref().map(|t| {
                PathBuf::from(
                    t.replace("{root}", &settings.sysfs_root)
                        .replace("{channel}", channel)
                        .replace("{index}", "1"),
                )
            }),
            _ => None,
        }
    };
    ["red", "green", "blue"]
        .iter()
        .all(|channel| sample(channel).is_some_and(|p| p.exists()))
}

async fn run_plugin(app: App, id: String, event: String, cancelled: Arc<AtomicBool>) -> Result<()> {
    let device = app.device.lock().await.clone();
    let led_count = app.led.settings.lock().await.count;
    let (manifest, root, allowed) = {
        let p = app.plugins.lock().await;
        if !p.enabled.contains(&id) {
            bail!("plugin is disabled")
        };
        let m = p.manifest(&id)?;
        if let Some(reason) = compatibility_error(&m, &device) {
            bail!("plugin is incompatible: {reason}")
        };
        (
            m,
            p.root.join(&id),
            app.settings.lock().await.allow_plugin_binaries,
        )
    };
    let led = app.led.clone();
    task::spawn_blocking(move || -> Result<()> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )?;
        let globals = lua.globals();
        let api = lua.create_table()?;
        let permissions: HashSet<_> = manifest.permissions.iter().map(String::as_str).collect();
        if permissions.contains("led") {
            let led2 = led.clone();
            let cancelled_set = cancelled.clone();
            api.set(
                "set",
                lua.create_function(move |_, colors: mlua::Table| {
                    if cancelled_set.load(Ordering::Relaxed) {
                        return Err(mlua::Error::external("plugin stopped"));
                    }
                    let v = colors
                        .sequence_values::<String>()
                        .collect::<mlua::Result<Vec<_>>>()?;
                    tokio::runtime::Handle::current()
                        .block_on(led2.draw(v))
                        .map_err(mlua::Error::external)
                })?,
            )?;
            let led3 = led.clone();
            api.set(
                "off",
                lua.create_function(move |_, ()| {
                    tokio::runtime::Handle::current()
                        .block_on(led3.off())
                        .map_err(mlua::Error::external)
                })?,
            )?;
            let pattern_root = root.clone();
            api.set(
                "play",
                lua.create_function(move |_, name: String| {
                    safe_relative(&name).map_err(mlua::Error::external)?;
                    tokio::runtime::Handle::current()
                        .block_on(led.play_led(pattern_root.join(name)))
                        .map_err(mlua::Error::external)
                })?,
            )?;
            let cancelled_sleep = cancelled.clone();
            api.set(
                "sleep",
                lua.create_function(move |_, ms: u64| {
                    if ms > 60_000 {
                        return Err(mlua::Error::external("sleep is limited to 60 seconds"));
                    };
                    let mut elapsed = 0;
                    while elapsed < ms {
                        if cancelled_sleep.load(Ordering::Relaxed) {
                            return Err(mlua::Error::external("plugin stopped"));
                        }
                        let slice = (ms - elapsed).min(20);
                        std::thread::sleep(Duration::from_millis(slice));
                        elapsed += slice;
                    }
                    Ok(())
                })?,
            )?;
        }
        if permissions.contains("volume") {
            let volume = lua.create_table()?;
            volume.set(
                "set",
                lua.create_function(|_, percent: u8| {
                    if percent > 100 {
                        return Err(mlua::Error::external("volume must be 0..100"));
                    };
                    command_output(
                        "amixer",
                        &["set".into(), "Master".into(), format!("{percent}%")],
                    )
                    .map_err(mlua::Error::external)
                })?,
            )?;
            volume.set(
                "get",
                lua.create_function(|_, ()| {
                    command_output("amixer", &["get".into(), "Master".into()])
                        .map_err(mlua::Error::external)
                })?,
            )?;
            api.set("volume", volume)?;
        }
        if permissions.contains("audio") {
            let audio = lua.create_table()?;
            let audio_root = root.clone();
            audio.set(
                "play",
                lua.create_function(move |_, file: String| {
                    safe_relative(&file).map_err(mlua::Error::external)?;
                    let path = audio_root.join(file);
                    if !path.is_file() {
                        return Err(mlua::Error::external("audio file not found"));
                    };
                    Command::new("aplay")
                        .arg("-q")
                        .arg(path)
                        .spawn()
                        .map(|_| ())
                        .map_err(mlua::Error::external)
                })?,
            )?;
            api.set("audio", audio)?;
        }
        if permissions.contains("bluetooth") {
            let bt = lua.create_table()?;
            bt.set(
                "status",
                lua.create_function(|_, ()| {
                    command_output("bluetoothctl", &["show".into()]).map_err(mlua::Error::external)
                })?,
            )?;
            bt.set(
                "power",
                lua.create_function(|_, on: bool| {
                    command_output(
                        "bluetoothctl",
                        &["power".into(), if on { "on".into() } else { "off".into() }],
                    )
                    .map_err(mlua::Error::external)
                })?,
            )?;
            bt.set(
                "scan",
                lua.create_function(|_, on: bool| {
                    command_output(
                        "bluetoothctl",
                        &["scan".into(), if on { "on".into() } else { "off".into() }],
                    )
                    .map_err(mlua::Error::external)
                })?,
            )?;
            for action in ["pair", "connect", "disconnect", "remove"] {
                let a = action.to_string();
                bt.set(
                    action,
                    lua.create_function(move |_, mac: String| {
                        if !valid_mac(&mac) {
                            return Err(mlua::Error::external("invalid Bluetooth MAC"));
                        };
                        command_output("bluetoothctl", &[a.clone(), mac])
                            .map_err(mlua::Error::external)
                    })?,
                )?;
            }
            api.set("bluetooth", bt)?;
        }
        if permissions.contains("display") {
            let display = lua.create_table()?;
            display.set(
                "backlights",
                lua.create_function(|lua, ()| {
                    let t = lua.create_table()?;
                    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
                        for (i, e) in entries.flatten().enumerate() {
                            t.set(i + 1, e.file_name().to_string_lossy().to_string())?
                        }
                    }
                    Ok(t)
                })?,
            )?;
            display.set(
                "set_backlight",
                lua.create_function(|_, (name, value): (String, u32)| {
                    validate_id(&name).map_err(mlua::Error::external)?;
                    let base = Path::new("/sys/class/backlight").join(&name);
                    let max: u32 = fs::read_to_string(base.join("max_brightness"))
                        .map_err(mlua::Error::external)?
                        .trim()
                        .parse()
                        .map_err(mlua::Error::external)?;
                    if value > max {
                        return Err(mlua::Error::external("brightness exceeds max_brightness"));
                    };
                    fs::write(base.join("brightness"), value.to_string())
                        .map_err(mlua::Error::external)
                })?,
            )?;
            api.set("display", display)?;
        }
        if permissions.contains("system") {
            let system = lua.create_table()?;
            system.set(
                "hostname",
                lua.create_function(|_, ()| {
                    fs::read_to_string("/etc/hostname")
                        .map(|x| x.trim().to_string())
                        .map_err(mlua::Error::external)
                })?,
            )?;
            system.set(
                "uptime",
                lua.create_function(|_, ()| {
                    fs::read_to_string("/proc/uptime").map_err(mlua::Error::external)
                })?,
            )?;
            system.set(
                "meminfo",
                lua.create_function(|_, ()| {
                    fs::read_to_string("/proc/meminfo").map_err(mlua::Error::external)
                })?,
            )?;
            api.set("system", system)?;
        }
        if permissions.contains("exec") {
            let plugin_root = root.clone();
            api.set(
                "exec",
                lua.create_function(move |_, (program, args): (String, Vec<String>)| {
                    if !allowed {
                        return Err(mlua::Error::external(
                            "binary execution disabled by administrator",
                        ));
                    };
                    safe_relative(&program).map_err(mlua::Error::external)?;
                    let output = Command::new(plugin_root.join(program))
                        .args(args)
                        .current_dir(&plugin_root)
                        .output()
                        .map_err(mlua::Error::external)?;
                    Ok(String::from_utf8_lossy(&output.stdout).to_string())
                })?,
            )?;
        }
        globals.set("alice", api)?;
        globals.set("event", event)?;
        let lua_device = lua.create_table()?;
        lua_device.set("model", device.model)?;
        lua_device.set("architecture", device.architecture)?;
        lua_device.set("led_count", led_count)?;
        let lua_features = lua.create_table()?;
        for (i, feature) in device.features.iter().enumerate() {
            lua_features.set(i + 1, feature.as_str())?;
        }
        lua_device.set("features", lua_features)?;
        globals.set("device", lua_device)?;
        let launch_path = manifest.launch.path(&manifest).to_string();
        let source = fs::read_to_string(root.join(&launch_path))?;
        lua.load(&source).set_name(&launch_path).exec()?;
        Ok(())
    })
    .await??;
    Ok(())
}

async fn run_binary(app: App, id: String, cancelled: Arc<AtomicBool>) -> Result<()> {
    let device = app.device.lock().await.clone();
    let (manifest, root) = {
        let plugins = app.plugins.lock().await;
        if !plugins.enabled.contains(&id) {
            bail!("package is disabled")
        }
        let manifest = plugins.manifest(&id)?;
        if let Some(reason) = compatibility_error(&manifest, &device) {
            bail!("package is incompatible: {reason}")
        }
        if !manifest
            .permissions
            .iter()
            .any(|permission| permission == "exec")
        {
            bail!("binary packages require the exec permission")
        }
        (manifest, plugins.root.join(&id))
    };
    task::spawn_blocking(move || -> Result<()> {
        let executable = manifest.launch.path(&manifest).to_string();
        safe_relative(&executable)?;
        let log_path = root.join("runtime.log");
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let mut child = Command::new(root.join(executable))
            .args(&manifest.launch.args)
            .current_dir(&root)
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                bail!("binary exited with {status}")
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await??;
    Ok(())
}

async fn run_package(app: App, id: String, cancelled: Arc<AtomicBool>) -> Result<()> {
    let kind = app.plugins.lock().await.manifest(&id)?.launch.kind;
    match kind {
        LaunchKind::Lua => run_plugin(app, id, "manual".into(), cancelled).await,
        LaunchKind::Binary => run_binary(app, id, cancelled).await,
    }
}

async fn start_package(app: App, id: String) -> Result<()> {
    let cancelled = {
        let mut plugins = app.plugins.lock().await;
        if !plugins.enabled.contains(&id) {
            bail!("package is disabled")
        }
        if let Some(previous) = plugins.running.remove(&id) {
            previous.store(true, Ordering::Relaxed)
        }
        let token = Arc::new(AtomicBool::new(false));
        plugins.running.insert(id.clone(), token.clone());
        token
    };
    let task_app = app.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        if let Err(error) = run_package(task_app.clone(), task_id.clone(), cancelled.clone()).await
        {
            if !cancelled.load(Ordering::Relaxed) {
                tracing::error!(package = %task_id, %error, "package execution failed")
            }
        }
        let mut plugins = task_app.plugins.lock().await;
        if plugins
            .running
            .get(&task_id)
            .is_some_and(|current| Arc::ptr_eq(current, &cancelled))
        {
            plugins.running.remove(&task_id);
        }
    });
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(UI)
}
async fn list(State(a): State<App>) -> Result<Json<Vec<PluginInfo>>, ApiError> {
    let d = a.device.lock().await.clone();
    Ok(Json(a.plugins.lock().await.list(&d)?))
}
async fn install(State(a): State<App>, mut form: Multipart) -> Result<Json<Manifest>, ApiError> {
    while let Some(f) = form.next_field().await? {
        if f.name() == Some("package") {
            let b = f.bytes().await?;
            let m = a.plugins.lock().await.install(&b)?;
            return Ok(Json(m));
        }
    }
    Err(ApiError(anyhow!("missing package field")))
}
async fn toggle(
    State(a): State<App>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let mut p = a.plugins.lock().await;
    let manifest = p.manifest(&id)?;
    let enabled = p.enabled.insert(id.clone());
    if !enabled {
        p.enabled.remove(&id);
    }
    p.save()?;
    drop(p);
    if enabled && manifest.launch.mode == LaunchMode::Service {
        start_package(a, id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn remove(
    State(a): State<App>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    a.plugins.lock().await.remove(&id)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn invoke(
    State(a): State<App>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    start_package(a, id).await?;
    Ok(StatusCode::ACCEPTED)
}
async fn stop_plugin(
    State(a): State<App>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    if let Some(token) = a.plugins.lock().await.running.remove(&id) {
        token.store(true, Ordering::Relaxed);
    }
    let _ = a.led.off().await;
    Ok(StatusCode::NO_CONTENT)
}
async fn package_logs(
    State(a): State<App>,
    AxumPath(id): AxumPath<String>,
) -> Result<String, ApiError> {
    validate_id(&id)?;
    let path = a.plugins.lock().await.root.join(id).join("runtime.log");
    Ok(fs::read_to_string(path).unwrap_or_default())
}
async fn get_settings(State(a): State<App>) -> Json<Settings> {
    Json(a.settings.lock().await.clone())
}
async fn get_device(State(a): State<App>) -> Json<DeviceProfile> {
    Json(a.device.lock().await.clone())
}
async fn put_device(
    State(a): State<App>,
    Json(d): Json<DeviceProfile>,
) -> Result<StatusCode, ApiError> {
    validate_id(&d.model)?;
    fs::write(a.root.join("device.toml"), toml::to_string_pretty(&d)?)?;
    *a.device.lock().await = d;
    Ok(StatusCode::NO_CONTENT)
}
async fn put_settings(
    State(a): State<App>,
    Json(s): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    if s.led.count == 0 || s.led.count > 128 {
        return Err(ApiError(anyhow!("LED count must be 1..128")));
    };
    fs::write(a.root.join("settings.toml"), toml::to_string_pretty(&s)?)?;
    *a.settings.lock().await = s.clone();
    *a.led.settings.lock().await = s.led;
    Ok(StatusCode::NO_CONTENT)
}
async fn led_set(State(a): State<App>, Json(f): Json<Frame>) -> Result<StatusCode, ApiError> {
    a.led.draw(f.colors).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn led_off(State(a): State<App>) -> Result<StatusCode, ApiError> {
    a.led.off().await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Debug)]
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let args = Args::parse();
    fs::create_dir_all(&args.data_dir)?;
    let settings_path = args.data_dir.join("settings.toml");
    let has_settings = settings_path.exists();
    let mut settings: Settings = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|x| toml::from_str(&x).ok())
        .unwrap_or_default();
    if !has_settings && Path::new("/sys/class/leds/micro:red:ring1/brightness").exists() {
        settings.led.profile = "micro".into();
        settings.led.count = 3;
    }
    let led = Arc::new(Led {
        settings: Arc::new(Mutex::new(settings.led.clone())),
    });
    let mut device = detect_device(&args.data_dir, args.model);
    if configured_rgb_ring(&settings.led) {
        device.features.push("led.ring.rgb".into());
        device
            .features
            .push(format!("led.ring.{}", settings.led.count));
        device.features.push("led.pattern.v1".into());
    }
    tracing::info!(model=%device.model,features=?device.features,"detected device");
    let app = App {
        root: args.data_dir.clone(),
        settings: Arc::new(Mutex::new(settings)),
        device: Arc::new(Mutex::new(device)),
        led,
        plugins: Arc::new(Mutex::new(PluginManager::new(
            args.data_dir.join("plugins"),
        )?)),
    };
    let services = {
        let device = app.device.lock().await.clone();
        app.plugins
            .lock()
            .await
            .list(&device)?
            .into_iter()
            .filter(|package| {
                package.enabled && package.manifest.launch.mode == LaunchMode::Service
            })
            .map(|package| package.manifest.id)
            .collect::<Vec<_>>()
    };
    for id in services {
        if let Err(error) = start_package(app.clone(), id.clone()).await {
            tracing::error!(package = %id, %error, "service package failed to start");
        }
    }
    let router = Router::new()
        .route("/", get(index))
        .route("/api/plugins", get(list).post(install))
        .route("/api/plugins/:id", delete(remove))
        .route("/api/plugins/:id/toggle", post(toggle))
        .route("/api/plugins/:id/invoke", post(invoke))
        .route("/api/plugins/:id/stop", post(stop_plugin))
        .route("/api/plugins/:id/logs", get(package_logs))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/device", get(get_device).put(put_device))
        .route("/api/led", post(led_set))
        .route("/api/led/off", post(led_off))
        .layer(TraceLayer::new_for_http())
        .with_state(app);
    let l = TcpListener::bind(&args.listen).await?;
    tracing::info!("web UI: http://{}", args.listen);
    axum::serve(l, router).await?;
    Ok(())
}
const UI: &str = include_str!("../web/index.html");
