use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use clap::Parser;
use flate2::read::GzDecoder;
use mlua::{Lua, LuaOptions, StdLib};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Cursor,
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
    web: WebSettings,
    #[serde(default)]
    allow_plugin_binaries: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSettings {
    #[serde(default = "default_true")]
    ui: bool,
    #[serde(default = "default_true")]
    api: bool,
}
fn default_true() -> bool {
    true
}
impl Default for WebSettings {
    fn default() -> Self {
        Self {
            ui: true,
            api: true,
        }
    }
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
            web: WebSettings::default(),
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
#[derive(Debug, Clone)]
struct Sensor {
    id: String,
    kind: String,
    path: PathBuf,
    unit: String,
}
#[derive(Debug, Serialize)]
struct SensorInfo {
    id: String,
    kind: String,
    unit: String,
    value: Option<f64>,
}
#[derive(Debug, Serialize)]
struct ClockInfo {
    available: bool,
    segments: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct ClockRequest {
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    brightness: Option<u8>,
    #[serde(default)]
    segments: Option<HashMap<String, u8>>,
    #[serde(default)]
    off: bool,
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
#[derive(Deserialize)]
struct Invocation {
    #[serde(default)]
    args: Vec<String>,
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
        let stage = self.root.join(format!(".install-{}", Uuid::new_v4()));
        fs::create_dir(&stage)?;
        let extract_result = if bytes.starts_with(b"PK\x03\x04") {
            extract_zip_yap(bytes, &stage)
        } else if bytes.starts_with(&[0x1f, 0x8b]) {
            extract_tar_gz_yap(bytes, &stage)
        } else {
            bail!(".yap must be a ZIP or gzip-compressed TAR archive")
        };
        if let Err(error) = extract_result {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
        let manifest_text = fs::read_to_string(stage.join("plugin.toml"))
            .context(".yap must contain root plugin.toml")?;
        let m: Manifest = toml::from_str(&manifest_text)?;
        validate_id(&m.id)?;
        safe_relative(&m.entry)?;
        safe_relative(m.launch.path(&m))?;
        if m.launch.kind == LaunchKind::Binary && !m.permissions.iter().any(|p| p == "exec") {
            bail!("binary packages require the exec permission")
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
fn archive_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("unsafe archive path")
    }
    Ok(path.to_path_buf())
}
fn set_archive_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}
fn extract_zip_yap(bytes: &[u8], stage: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("unsafe archive path"))?
            .to_owned();
        let destination = stage.join(path);
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mode = entry.unix_mode();
            let mut output = fs::File::create(&destination)?;
            std::io::copy(&mut entry, &mut output)?;
            if let Some(mode) = mode {
                set_archive_mode(&destination, mode)?;
            }
        }
    }
    Ok(())
}
fn extract_tar_gz_yap(bytes: &[u8], stage: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    for item in archive.entries()? {
        let mut entry = item?;
        let path = archive_path(&entry.path()?)?;
        let destination = stage.join(path);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(destination)?;
        } else if entry_type.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mode = entry.header().mode()?;
            let mut output = fs::File::create(&destination)?;
            std::io::copy(&mut entry, &mut output)?;
            set_archive_mode(&destination, mode)?;
        } else {
            bail!("unsupported TAR archive entry")
        }
    }
    Ok(())
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
    let sensors = discover_sensors();
    if sensors.iter().any(|sensor| sensor.kind == "ambient_light") {
        features.push("sensor.ambient_light.iio".into())
    }
    if sensors.iter().any(|sensor| sensor.kind == "temperature") {
        features.push("sensor.temperature.sysfs".into())
    }
    if sensors.iter().any(|sensor| sensor.kind == "battery") {
        features.push("battery.sysfs".into())
    }
    if clock_available() {
        features.push("display.clock.segment.v1".into())
    }
    if arch.starts_with("arm") {
        features.push("arch.armhf".into())
    };
    if arch == "aarch64" {
        features.push("arch.aarch64".into())
    }
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
fn read_trimmed(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_string())
}
fn sorted_directories(path: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(path)
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}
fn discover_sensors() -> Vec<Sensor> {
    let mut sensors = Vec::new();
    for device in sorted_directories(Path::new("/sys/bus/iio/devices")) {
        let value = device.join("in_intensity_input");
        if value.is_file() {
            let name = device.file_name().unwrap_or_default().to_string_lossy();
            sensors.push(Sensor {
                id: format!("ambient-light:{name}"),
                kind: "ambient_light".into(),
                path: value,
                unit: "lux".into(),
            });
        }
    }
    for zone in sorted_directories(Path::new("/sys/class/thermal")) {
        let value = zone.join("temp");
        if value.is_file() {
            let name = zone.file_name().unwrap_or_default().to_string_lossy();
            sensors.push(Sensor {
                id: format!("temperature:{name}"),
                kind: "temperature".into(),
                path: value,
                unit: "celsius".into(),
            });
        }
    }
    for device in sorted_directories(Path::new("/sys/class/hwmon")) {
        let name = read_trimmed(&device.join("name")).unwrap_or_else(|_| {
            device
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        if let Ok(entries) = fs::read_dir(&device) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with("temp") && file_name.ends_with("_input") {
                    sensors.push(Sensor {
                        id: format!("temperature:{name}:{file_name}"),
                        kind: "temperature".into(),
                        path: entry.path(),
                        unit: "celsius".into(),
                    });
                }
            }
        }
    }
    for supply in sorted_directories(Path::new("/sys/class/power_supply")) {
        let value = supply.join("capacity");
        if value.is_file() {
            let name = supply.file_name().unwrap_or_default().to_string_lossy();
            sensors.push(Sensor {
                id: format!("battery:{name}"),
                kind: "battery".into(),
                path: value,
                unit: "percent".into(),
            });
        }
    }
    sensors
}
fn sensor_value(sensor: &Sensor) -> Result<f64> {
    let mut value: f64 = read_trimmed(&sensor.path)?.parse()?;
    if sensor.kind == "temperature" && value.abs() > 1_000.0 {
        value /= 1_000.0;
    }
    Ok(value)
}
fn configured_light_sensor(name: &str) -> Result<PathBuf> {
    let sensor = discover_sensors()
        .into_iter()
        .find(|sensor| sensor.id == format!("ambient-light:{name}"))
        .context("ambient light sensor not found")?;
    Ok(sensor
        .path
        .parent()
        .context("invalid ambient light sensor path")?
        .to_path_buf())
}
fn clock_path(segment: &str) -> PathBuf {
    Path::new("/sys/class/leds").join(format!("mini2:white:clock{segment}/brightness"))
}
fn clock_segments() -> Vec<String> {
    let mut segments = Vec::new();
    for position in 1..=4 {
        for part in ["A", "B", "C", "D", "E", "F", "G"] {
            segments.push(format!("{part}{position}"));
        }
    }
    segments.extend([
        "PLUS1".into(),
        "PLUS2".into(),
        "MINUS".into(),
        "DEGREE".into(),
    ]);
    segments
}
fn clock_available() -> bool {
    clock_segments()
        .iter()
        .all(|segment| clock_path(segment).is_file())
}
fn clock_write(segments: &HashMap<String, u8>) -> Result<()> {
    if !clock_available() {
        bail!("segment clock is not available")
    }
    for segment in clock_segments() {
        let value = segments.get(&segment).copied().unwrap_or(0);
        fs::write(clock_path(&segment), value.to_string())?;
    }
    Ok(())
}
fn validate_clock_segments(segments: &HashMap<String, u8>) -> Result<()> {
    let valid = clock_segments().into_iter().collect::<HashSet<_>>();
    if segments.keys().any(|segment| !valid.contains(segment)) {
        bail!("unknown clock segment")
    }
    Ok(())
}
fn clock_time(value: &str, brightness: u8) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![bytes[0], bytes[1], bytes[3], bytes[4]]
            .iter()
            .all(u8::is_ascii_digit)
    {
        bail!("clock time must have HH:MM format")
    }
    let digits: HashMap<u8, &[&str]> = HashMap::from([
        (b'0', &["A", "B", "C", "D", "E", "F"] as &[&str]),
        (b'1', &["B", "C"]),
        (b'2', &["A", "B", "G", "E", "D"]),
        (b'3', &["A", "B", "C", "D", "G"]),
        (b'4', &["F", "G", "B", "C"]),
        (b'5', &["A", "F", "G", "C", "D"]),
        (b'6', &["A", "F", "G", "E", "C", "D"]),
        (b'7', &["A", "B", "C"]),
        (b'8', &["A", "B", "C", "D", "E", "F", "G"]),
        (b'9', &["A", "B", "C", "D", "F", "G"]),
    ]);
    let mut segments = HashMap::new();
    for (index, digit) in [bytes[0], bytes[1], bytes[3], bytes[4]].iter().enumerate() {
        for part in digits.get(digit).context("invalid clock digit")?.iter() {
            segments.insert(format!("{part}{}", index + 1), brightness);
        }
    }
    segments.insert("PLUS1".into(), brightness);
    segments.insert("PLUS2".into(), brightness);
    clock_write(&segments)
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
        if permissions.contains("sensors") {
            let sensors = lua.create_table()?;
            sensors.set(
                "list",
                lua.create_function(|lua, ()| {
                    let result = lua.create_table()?;
                    for (index, sensor) in discover_sensors().iter().enumerate() {
                        let item = lua.create_table()?;
                        item.set("id", sensor.id.as_str())?;
                        item.set("kind", sensor.kind.as_str())?;
                        item.set("unit", sensor.unit.as_str())?;
                        result.set(index + 1, item)?;
                    }
                    Ok(result)
                })?,
            )?;
            sensors.set(
                "read",
                lua.create_function(|lua, id: String| {
                    let sensor = discover_sensors()
                        .into_iter()
                        .find(|sensor| sensor.id == id)
                        .context("sensor not found")
                        .map_err(mlua::Error::external)?;
                    let result = lua.create_table()?;
                    result.set("id", sensor.id.as_str())?;
                    result.set("kind", sensor.kind.as_str())?;
                    result.set("unit", sensor.unit.as_str())?;
                    result.set(
                        "value",
                        sensor_value(&sensor).map_err(mlua::Error::external)?,
                    )?;
                    Ok(result)
                })?,
            )?;
            if permissions.contains("sensors.control") {
                sensors.set(
                    "configure_ambient_light",
                    lua.create_function(|_, (name, gain, integration_ms): (String, u8, u16)| {
                        if ![1, 3, 6, 9, 18].contains(&gain) {
                            return Err(mlua::Error::external("gain must be 1, 3, 6, 9 or 18"));
                        }
                        if ![25, 50, 100, 200, 400].contains(&integration_ms) {
                            return Err(mlua::Error::external(
                                "integration time must be 25, 50, 100, 200 or 400 ms",
                            ));
                        }
                        let path = configured_light_sensor(&name).map_err(mlua::Error::external)?;
                        fs::write(path.join("in_intensity_hardwaregain"), gain.to_string())
                            .map_err(mlua::Error::external)?;
                        fs::write(
                            path.join("in_intensity_integration_time"),
                            format!("0.{integration_ms:03}"),
                        )
                        .map_err(mlua::Error::external)
                    })?,
                )?;
            }
            api.set("sensors", sensors)?;
        }
        if permissions.contains("clock") {
            let clock = lua.create_table()?;
            clock.set(
                "available",
                lua.create_function(|_, ()| Ok(clock_available()))?,
            )?;
            clock.set(
                "segments",
                lua.create_function(|_, levels: mlua::Table| {
                    let mut frame = HashMap::new();
                    for pair in levels.pairs::<String, u8>() {
                        let (segment, value) = pair?;
                        frame.insert(segment, value);
                    }
                    validate_clock_segments(&frame).map_err(mlua::Error::external)?;
                    clock_write(&frame).map_err(mlua::Error::external)
                })?,
            )?;
            clock.set(
                "time",
                lua.create_function(|_, (time, brightness): (String, Option<u8>)| {
                    clock_time(&time, brightness.unwrap_or(170)).map_err(mlua::Error::external)
                })?,
            )?;
            clock.set(
                "off",
                lua.create_function(|_, ()| {
                    clock_write(&HashMap::new()).map_err(mlua::Error::external)
                })?,
            )?;
            api.set("clock", clock)?;
        }
        if permissions.contains("input") {
            let input = lua.create_table()?;
            input.set(
                "devices",
                lua.create_function(|_, ()| {
                    fs::read_to_string("/proc/bus/input/devices").map_err(mlua::Error::external)
                })?,
            )?;
            api.set("input", input)?;
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

async fn run_binary(
    app: App,
    id: String,
    invocation_args: Vec<String>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
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
            .args(invocation_args)
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

async fn run_package(
    app: App,
    id: String,
    invocation_args: Vec<String>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let kind = app.plugins.lock().await.manifest(&id)?.launch.kind;
    match kind {
        LaunchKind::Lua => run_plugin(app, id, "manual".into(), cancelled).await,
        LaunchKind::Binary => run_binary(app, id, invocation_args, cancelled).await,
    }
}

async fn start_package(app: App, id: String, invocation_args: Vec<String>) -> Result<()> {
    let cancelled = {
        let mut plugins = app.plugins.lock().await;
        if !plugins.enabled.contains(&id) {
            bail!("package is disabled")
        }
        let manifest = plugins.manifest(&id)?;
        if !invocation_args.is_empty() {
            if manifest.launch.kind != LaunchKind::Binary {
                bail!("only binary packages accept invocation arguments")
            }
            if manifest.launch.mode != LaunchMode::Oneshot {
                bail!("invocation arguments are only supported for oneshot packages")
            }
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
        if let Err(error) = run_package(
            task_app.clone(),
            task_id.clone(),
            invocation_args,
            cancelled.clone(),
        )
        .await
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
        start_package(a, id, Vec::new()).await?;
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
    invocation: Option<Json<Invocation>>,
) -> Result<StatusCode, ApiError> {
    start_package(
        a,
        id,
        invocation.map(|value| value.0.args).unwrap_or_default(),
    )
    .await?;
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
async fn get_sensors() -> Json<Vec<SensorInfo>> {
    Json(
        discover_sensors()
            .into_iter()
            .map(|sensor| SensorInfo {
                value: sensor_value(&sensor).ok(),
                id: sensor.id,
                kind: sensor.kind,
                unit: sensor.unit,
            })
            .collect(),
    )
}
async fn get_clock() -> Json<ClockInfo> {
    Json(ClockInfo {
        available: clock_available(),
        segments: clock_segments(),
    })
}
async fn put_clock(Json(request): Json<ClockRequest>) -> Result<StatusCode, ApiError> {
    if request.off {
        clock_write(&HashMap::new())?;
    } else if let Some(time) = request.time {
        clock_time(&time, request.brightness.unwrap_or(170))?;
    } else if let Some(segments) = request.segments {
        validate_clock_segments(&segments)?;
        clock_write(&segments)?;
    } else {
        return Err(ApiError(anyhow!("pass time, segments or off=true")));
    }
    Ok(StatusCode::NO_CONTENT)
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
    if s.web.ui && !s.web.api {
        return Err(ApiError(anyhow!("web.ui requires web.api")));
    }
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
        if let Err(error) = start_package(app.clone(), id.clone(), Vec::new()).await {
            tracing::error!(package = %id, %error, "service package failed to start");
        }
    }
    let web = app.settings.lock().await.web.clone();
    if web.ui && !web.api {
        bail!("web.ui requires web.api")
    }
    if !web.ui && !web.api {
        tracing::info!("web interface is disabled");
        std::future::pending::<()>().await;
    }
    let mut router = Router::new();
    if web.ui {
        router = router.route("/", get(index));
    }
    if web.api {
        router = router
            .route("/api/plugins", get(list).post(install))
            .route("/api/plugins/:id", delete(remove))
            .route("/api/plugins/:id/toggle", post(toggle))
            .route("/api/plugins/:id/invoke", post(invoke))
            .route("/api/plugins/:id/stop", post(stop_plugin))
            .route("/api/plugins/:id/logs", get(package_logs))
            .route("/api/settings", get(get_settings).put(put_settings))
            .route("/api/device", get(get_device).put(put_device))
            .route("/api/sensors", get(get_sensors))
            .route("/api/clock", get(get_clock).put(put_clock))
            .route("/api/led", post(led_set))
            .route("/api/led/off", post(led_off));
    }
    let router = router.layer(TraceLayer::new_for_http()).with_state(app);
    let l = TcpListener::bind(&args.listen).await?;
    tracing::info!("web UI: http://{}", args.listen);
    axum::serve(l, router).await?;
    Ok(())
}
const UI: &str = include_str!("../web/index.html");
