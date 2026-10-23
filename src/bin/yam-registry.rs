use anyhow::{anyhow, bail, Context, Result};
use aws_credential_types::Credentials;
use aws_sdk_s3::{primitives::ByteStream, Client as S3Client};
use aws_types::region::Region;
use axum::{
    body::Body,
    extract::{Multipart, Path as AxumPath, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const MAX_PACKAGE_BYTES: usize = 50 * 1024 * 1024;

#[derive(Parser)]
#[command(name = "yam-registry", about = "YAM package registry")]
struct Args {
    #[arg(long, env = "YAM_REGISTRY_LISTEN", default_value = "0.0.0.0:8100")]
    listen: String,
    #[arg(long, env = "YAM_REGISTRY_DATA_DIR", default_value = "./registry-data")]
    data_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    s3_bucket: String,
    #[serde(default = "default_region")]
    s3_region: String,
    #[serde(default)]
    s3_endpoint: Option<String>,
    hcaptcha_sitekey: String,
    #[serde(default)]
    session_secure: bool,
}
fn default_region() -> String {
    "us-east-1".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    entry: String,
    #[serde(default)]
    description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackageRecord {
    manifest: Manifest,
    object_key: String,
    sha256: String,
    size: usize,
    published_at: u64,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct PackageIndex {
    packages: Vec<PackageRecord>,
}
struct App {
    config: Config,
    hcaptcha_secret: String,
    s3: S3Client,
    index_path: PathBuf,
    index: Mutex<PackageIndex>,
    sessions: Mutex<HashMap<String, SystemTime>>,
    http: reqwest::Client,
}
type SharedApp = Arc<App>;

#[derive(Deserialize)]
struct CaptchaRequest {
    token: String,
}
#[derive(Deserialize)]
struct CaptchaResponse {
    success: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let args = Args::parse();
    fs::create_dir_all(&args.data_dir)?;
    let config: Config = toml::from_str(&fs::read_to_string(args.data_dir.join("registry.toml"))?)
        .context("read registry.toml")?;
    let hcaptcha_secret = std::env::var("HCAPTCHA_SECRET")
        .context("HCAPTCHA_SECRET must be set; do not store it in registry.toml")?;
    let access_key = std::env::var("AWS_ACCESS_KEY_ID").context("AWS_ACCESS_KEY_ID must be set")?;
    let secret_key =
        std::env::var("AWS_SECRET_ACCESS_KEY").context("AWS_SECRET_ACCESS_KEY must be set")?;
    let credentials = Credentials::new(access_key, secret_key, None, None, "yam-registry");
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new(config.s3_region.clone()))
        .credentials_provider(credentials);
    if let Some(endpoint) = &config.s3_endpoint {
        loader = loader.endpoint_url(endpoint);
    }
    let sdk_config = loader.load().await;
    let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
        .force_path_style(config.s3_endpoint.is_some())
        .build();
    let s3 = S3Client::from_conf(s3_config);
    let index_path = args.data_dir.join("index.json");
    let index = fs::read_to_string(&index_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let app = Arc::new(App {
        config,
        hcaptcha_secret,
        s3,
        index_path,
        index: Mutex::new(index),
        sessions: Mutex::new(HashMap::new()),
        http: reqwest::Client::new(),
    });
    let router = Router::new()
        .route("/", get(web_ui))
        .route("/api/config", get(public_config))
        .route("/api/auth/hcaptcha", post(authorize))
        .route("/api/packages", get(list_packages).post(upload_package))
        .route("/api/packages/:id/:version/download", get(download_package))
        .layer(TraceLayer::new_for_http())
        .with_state(app);
    let listener = TcpListener::bind(&args.listen).await?;
    tracing::info!(address = %args.listen, "YAM registry is listening");
    axum::serve(listener, router).await?;
    Ok(())
}

async fn web_ui(State(app): State<SharedApp>, headers: HeaderMap) -> Html<String> {
    if has_session(&app, &headers).await {
        Html(REGISTRY_UI.to_string())
    } else {
        Html(LOGIN_UI.replace("__HCAPTCHA_SITEKEY__", &app.config.hcaptcha_sitekey))
    }
}
async fn public_config(State(app): State<SharedApp>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"hcaptcha_sitekey": app.config.hcaptcha_sitekey}))
}
async fn authorize(
    State(app): State<SharedApp>,
    Json(request): Json<CaptchaRequest>,
) -> Result<Response, ApiError> {
    verify_hcaptcha(&app, &request.token).await?;
    let session = Uuid::new_v4().to_string();
    app.sessions.lock().await.insert(
        session.clone(),
        SystemTime::now() + Duration::from_secs(3600),
    );
    let secure = if app.config.session_secure {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "yam_registry_session={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age=3600{secure}"
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError(anyhow!("invalid session cookie")))?,
    );
    Ok(response)
}
async fn list_packages(State(app): State<SharedApp>) -> Json<Vec<PackageRecord>> {
    let mut packages = app.index.lock().await.packages.clone();
    packages.sort_by(|left, right| right.published_at.cmp(&left.published_at));
    Json(packages)
}
async fn upload_package(
    State(app): State<SharedApp>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Result<Json<PackageRecord>, ApiError> {
    let mut captcha = None;
    let mut package = None;
    while let Some(field) = form.next_field().await? {
        match field.name() {
            Some("h-captcha-response") => captcha = Some(field.text().await?),
            Some("package") => {
                let bytes = field.bytes().await?;
                if bytes.len() > MAX_PACKAGE_BYTES {
                    return Err(ApiError(anyhow!("package exceeds 50 MiB limit")));
                }
                package = Some(bytes.to_vec());
            }
            _ => {}
        }
    }
    if !has_session(&app, &headers).await {
        verify_hcaptcha(&app, &captcha.context("h-captcha-response is required")?).await?;
    }
    let bytes = package.context("package field is required")?;
    let manifest = read_manifest(&bytes)?;
    validate_manifest(&manifest)?;
    let sha256 = sha256_hex(&bytes);
    let object_key = format!(
        "packages/{}/{}/{}.yap",
        manifest.id, manifest.version, sha256
    );
    let record = PackageRecord {
        manifest,
        object_key: object_key.clone(),
        sha256,
        size: bytes.len(),
        published_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    {
        let index = app.index.lock().await;
        if index.packages.iter().any(|existing| {
            existing.manifest.id == record.manifest.id
                && existing.manifest.version == record.manifest.version
        }) {
            return Err(ApiError(anyhow!("package version already exists")));
        }
    }
    app.s3
        .put_object()
        .bucket(&app.config.s3_bucket)
        .key(&object_key)
        .content_type("application/vnd.yam.package")
        .body(ByteStream::from(bytes))
        .send()
        .await?;
    let mut index = app.index.lock().await;
    if index.packages.iter().any(|existing| {
        existing.manifest.id == record.manifest.id
            && existing.manifest.version == record.manifest.version
    }) {
        return Err(ApiError(anyhow!("package version already exists")));
    }
    index.packages.push(record.clone());
    save_index(&app.index_path, &index)?;
    Ok(Json(record))
}
async fn download_package(
    State(app): State<SharedApp>,
    AxumPath((id, version)): AxumPath<(String, String)>,
) -> Result<Response, ApiError> {
    let record = app
        .index
        .lock()
        .await
        .packages
        .iter()
        .find(|record| record.manifest.id == id && record.manifest.version == version)
        .cloned()
        .context("package version not found")?;
    let object = app
        .s3
        .get_object()
        .bucket(&app.config.s3_bucket)
        .key(record.object_key)
        .send()
        .await?;
    let bytes = object.body.collect().await?.into_bytes();
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.yam.package"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{}-{}.yap\"", id, version),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}
async fn verify_hcaptcha(app: &App, token: &str) -> Result<()> {
    if token.trim().is_empty() {
        bail!("hCaptcha token is required")
    }
    let response: CaptchaResponse = app
        .http
        .post("https://hcaptcha.com/siteverify")
        .form(&[
            ("secret", app.hcaptcha_secret.as_str()),
            ("response", token),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !response.success {
        bail!("hCaptcha verification failed")
    }
    Ok(())
}
async fn has_session(app: &App, headers: &HeaderMap) -> bool {
    let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(session) = cookie
        .split(';')
        .map(str::trim)
        .find_map(|entry| entry.strip_prefix("yam_registry_session="))
    else {
        return false;
    };
    let mut sessions = app.sessions.lock().await;
    sessions.retain(|_, expiry| *expiry > SystemTime::now());
    sessions
        .get(session)
        .is_some_and(|expiry| *expiry > SystemTime::now())
}
fn read_manifest(bytes: &[u8]) -> Result<Manifest> {
    let raw = if bytes.starts_with(b"PK\x03\x04") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        let mut file = archive
            .by_name("plugin.toml")
            .context("missing root plugin.toml")?;
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        raw
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        let decoder = GzDecoder::new(Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        let mut raw = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            if safe_archive_path(&entry.path()?)? == Path::new("plugin.toml") {
                let mut value = String::new();
                entry.read_to_string(&mut value)?;
                raw = Some(value);
                break;
            }
        }
        raw.context("missing root plugin.toml")?
    } else {
        bail!(".yap must be a ZIP or gzip-compressed TAR archive")
    };
    Ok(toml::from_str(&raw)?)
}
fn safe_archive_path(path: &Path) -> Result<&Path> {
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("unsafe archive path")
    }
    Ok(path)
}
fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.id.is_empty()
        || !manifest
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("invalid package id")
    }
    if manifest.version.is_empty() || manifest.entry.is_empty() {
        bail!("manifest version and entry are required")
    }
    Ok(())
}
fn save_index(path: &Path, index: &PackageIndex) -> Result<()> {
    let staging = path.with_extension("json.new");
    fs::write(&staging, serde_json::to_vec_pretty(index)?)?;
    fs::rename(staging, path)?;
    Ok(())
}
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
#[derive(Debug)]
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}
const LOGIN_UI: &str = include_str!("../../web/registry-login.html");
const REGISTRY_UI: &str = include_str!("../../web/registry.html");
