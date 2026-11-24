//! Native component discovery and installation.
//!
//! The plugin package is a WebAssembly component with no filesystem access, so
//! the conversion kernel cannot live inside it: the sandbox has no preopens and
//! a 32 MiB memory ceiling. BMCBL loads a separately approved native library for
//! the host platform instead, and this module fetches the right one.
//!
//! Discovery order is the same as the rest of the ecosystem: the plugin registry
//! first (it is China-friendly and pins a digest per platform), then the GitHub
//! release index, then a component baked into the package. Every path ends with a
//! SHA-256 that BMCBL verifies before the library is loaded.

use bmcbl_plugin_api::app::{Job, VerifiedDownload, fetch_download_text};
use bmcbl_plugin_api::native::{self, Platform};
use bmcbl_plugin_api::prelude::*;
use semver::Version;
use serde::Deserialize;

/// Registry detail document for this plugin.
pub(super) const REGISTRY_DETAIL: &str =
    "https://bmcbl-plugin.chlna6666.com/v1/plugins/worldport.json";
/// Release index published beside every GitHub release.
const RELEASE_INDEX: &str = concat!(
    "https://github.com/BMCBL/WorldPort/releases/download/v",
    env!("CARGO_PKG_VERSION"),
    "/worldport-native.json"
);
const RELEASE_DOWNLOAD: &str = "https://github.com/BMCBL/WorldPort/releases/download";
const REGISTRY_ORIGIN: &str = "https://bmcbl-plugin.chlna6666.com";
const PLUGIN_ID: &str = "worldport";
const REPOSITORY: &str = "BMCBL/WorldPort";

/// Largest native library this plugin will install.
const MAX_COMPONENT_BYTES: u64 = 256 * 1024 * 1024;

/// A native component that matches the host platform.
pub(super) struct Component {
    /// Version of the release the library came from.
    pub(super) version: Version,
    /// SHA-256 the host verifies before loading the library.
    sha256: String,
    /// Verified download to start.
    request: Option<VerifiedDownload>,
}

impl Component {
    /// The digest this component installs under.
    pub(super) fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The verified download describing where to fetch it.
    pub(super) fn request(&self) -> Option<&VerifiedDownload> {
        self.request.as_ref()
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct RegistryRelease {
    schema_version: u32,
    id: String,
    repository: String,
    status: String,
    version: String,
    #[serde(default)]
    native_assets: Vec<RegistryAsset>,
}

#[derive(Deserialize)]
struct RegistryAsset {
    abi_major: u32,
    os: String,
    arch: String,
    environment: String,
    filename: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    sources: Vec<RegistrySource>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RegistrySource {
    Edgeone { path: String },
    Github { url: String },
    #[serde(other)]
    Other,
}

/// Requests the registry detail document.
pub(super) fn fetch_registry() -> PluginResult<Job<String>> {
    fetch_download_text(REGISTRY_DETAIL)
}

/// Requests the GitHub release index, used when the registry is unreachable.
pub(super) fn fetch_release_index() -> PluginResult<Job<String>> {
    fetch_download_text(RELEASE_INDEX)
}

/// Parses a registry detail document into a component for `platform`.
pub(super) fn registry_component(
    text: &str,
    platform: &Platform,
) -> PluginResult<Component> {
    let invalid = || plugin_error("native-release", "插件目录中的原生组件信息无效。");
    let release: RegistryRelease = serde_json::from_str(text).map_err(|_| invalid())?;
    if release.schema_version != 1
        || release.id != PLUGIN_ID
        || release.repository != REPOSITORY
        || release.status != "official"
    {
        return Err(invalid());
    }
    let version = Version::parse(&release.version)
        .ok()
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
        .ok_or_else(invalid)?;
    if version != Version::parse(env!("CARGO_PKG_VERSION")).expect("package version") {
        return Err(plugin_error("native-version", "插件目录版本与当前插件不一致，将使用对应 GitHub 发布。"));
    }
    let filename = component_filename(platform)?;
    let asset = release
        .native_assets
        .into_iter()
        .find(|asset| {
            asset.abi_major == native::abi::MAJOR
                && asset.os == platform.os
                && asset.arch == platform.arch
                && asset.environment == platform.environment
                && asset.filename == filename
                && valid_digest(&asset.sha256)
                && (1..=MAX_COMPONENT_BYTES).contains(&asset.size)
        })
        .ok_or_else(invalid)?;
    // Only accept the exact locations the registry is allowed to publish, so a
    // compromised document cannot point the download elsewhere.
    let github = format!("{RELEASE_DOWNLOAD}/v{version}/{filename}");
    let mirror = mirror_path(&asset.sha256, &filename);
    let mut has_mirror = false;
    let mut has_github = false;
    for source in asset.sources {
        match source {
            RegistrySource::Edgeone { path } if path == mirror => has_mirror = true,
            RegistrySource::Github { url } if url == github => has_github = true,
            _ => {}
        }
    }
    if !has_github {
        return Err(invalid());
    }
    // Prefer the verified CDN regardless of the source ordering in the index.
    // Duplicate records must not trigger duplicate network requests.
    let mut urls = Vec::with_capacity(2);
    if has_mirror {
        urls.push(format!("{REGISTRY_ORIGIN}{mirror}"));
    }
    urls.push(github);
    Ok(Component {
        version,
        sha256: asset.sha256.clone(),
        request: Some(VerifiedDownload {
            urls,
            filename,
            sha256: asset.sha256,
            size: asset.size,
        }),
    })
}

/// Parses a GitHub release index into a component for `platform`.
pub(super) fn release_component(text: &str, platform: &Platform) -> PluginResult<Component> {
    let release: Release = serde_json::from_str(text)
        .map_err(|error| plugin_error("native-release", error.to_string()))?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .and_then(|tag| Version::parse(tag).ok())
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
        .ok_or_else(|| {
            plugin_error(
                "native-version",
                "发布标签必须使用稳定的 vMAJOR.MINOR.PATCH 格式。",
            )
        })?;
    if version != Version::parse(env!("CARGO_PKG_VERSION")).expect("package version") {
        return Err(plugin_error("native-version", "原生组件发布版本与当前插件不一致。"));
    }
    let filename = component_filename(platform)?;
    let wanted = filename.clone();
    let missing = || {
        plugin_error(
            "native-asset",
            format!("发布 {version} 尚无已校验的 {filename}。"),
        )
    };
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == wanted)
        .ok_or_else(missing)?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|sha| valid_digest(sha))
        .ok_or_else(missing)?
        .to_owned();
    let url = format!("{RELEASE_DOWNLOAD}/v{version}/{filename}");
    let request = asset
        .size
        .filter(|size| (1..=MAX_COMPONENT_BYTES).contains(size))
        .map(|size| VerifiedDownload {
            // The digest is checked by the plugin against its packaged
            // component table, and by the host against the downloaded bytes.
            // Try the canonical immutable CDN path even when discovery fell
            // back to GitHub; an absent mirror simply falls back to GitHub.
            urls: vec![
                format!("{REGISTRY_ORIGIN}{}", mirror_path(&sha256, &wanted)),
                url,
            ],
            filename: wanted.clone(),
            sha256: sha256.clone(),
            size,
        });
    let request = request.ok_or_else(missing)?;
    Ok(Component {
        version,
        sha256,
        request: Some(request),
    })
}

/// Whether this host has a component name at all.
fn component_filename(platform: &Platform) -> PluginResult<String> {
    let extension = match (platform.os.as_str(), platform.environment.as_str()) {
        ("windows", "msvc") => "dll",
        ("linux", "gnu" | "musl") => "so",
        _ => {
            return Err(plugin_error(
                "native-platform",
                format!(
                    "当前平台 {} {} {} 没有可用的原生组件。",
                    platform.os, platform.arch, platform.environment,
                ),
            ));
        }
    };
    if !matches!(platform.arch.as_str(), "x86_64" | "aarch64") {
        return Err(plugin_error(
            "native-platform",
            format!("暂不支持 {} 架构的原生组件。", platform.arch),
        ));
    }
    Ok(format!(
        "{PLUGIN_ID}-native-abi{}-{}-{}-{}.{extension}",
        native::abi::MAJOR,
        platform.os,
        platform.arch,
        platform.environment
    ))
}

/// Matches Plugin-Registry's official_mirror_path: <sha256>.<extension>.
fn mirror_path(sha: &str, filename: &str) -> String {
    let extension = filename.rsplit('.').next().expect("native filename extension");
    format!("/downloads/official/{PLUGIN_ID}/{sha}.{extension}")
}

fn valid_digest(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(os: &str, arch: &str, env: &str) -> Platform {
        Platform {
            os: os.to_owned(),
            arch: arch.to_owned(),
            environment: env.to_owned(),
        }
    }

    #[test]
    fn release_index_matches_running_plugin_and_rejects_other_versions() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(RELEASE_INDEX.contains(&format!("/v{version}/")));
        let sha = "a".repeat(64);
        let metadata = serde_json::json!({
            "tag_name": format!("v{version}"),
            "assets": [{
                "name": "worldport-native-abi1-windows-x86_64-msvc.dll",
                "digest": format!("sha256:{sha}"),
                "size": 1024
            }]
        });
        let host = platform("windows", "x86_64", "msvc");
        let component = release_component(&metadata.to_string(), &host).expect("matching release");
        assert_eq!(component.sha256(), sha);
        assert!(component.request().is_some());
        let mut wrong = metadata;
        wrong["tag_name"] = serde_json::json!("v99.0.0");
        assert!(release_component(&wrong.to_string(), &host).is_err());
    }

    #[test]
    fn official_registry_native_mirrors_are_preferred_for_all_release_platforms() {
        let version = env!("CARGO_PKG_VERSION");
        let sha = "a".repeat(64);
        for (os, arch, env, extension) in [
            ("windows", "x86_64", "msvc", "dll"),
            ("windows", "aarch64", "msvc", "dll"),
            ("linux", "x86_64", "gnu", "so"),
            ("linux", "aarch64", "gnu", "so"),
        ] {
            let host = platform(os, arch, env);
            let filename = component_filename(&host).unwrap();
            let github = format!("{RELEASE_DOWNLOAD}/v{version}/{filename}");
            let mirror = format!("/downloads/official/worldport/{sha}.{extension}");
            let metadata = serde_json::json!({
                "schema_version": 1, "id": "worldport", "repository": REPOSITORY,
                "status": "official", "version": version,
                "native_assets": [{
                    "abi_major": 1, "os": os, "arch": arch, "environment": env,
                    "filename": filename, "size": 1024, "sha256": sha,
                    "sources": [
                        {"type": "github", "url": github},
                        {"type": "edgeone", "path": mirror},
                        {"type": "edgeone", "path": mirror}
                    ]
                }]
            });
            let component = registry_component(&metadata.to_string(), &host).expect("official mirror");
            assert_eq!(component.request().unwrap().urls, vec![format!("{REGISTRY_ORIGIN}{mirror}"), github.clone()]);
            let mut wrong = metadata;
            wrong["native_assets"][0]["sources"] = serde_json::json!([
                {"type": "edgeone", "path": format!("/downloads/official/worldport/{sha}.{filename}")},
                {"type": "github", "url": github}
            ]);
            let component = registry_component(&wrong.to_string(), &host).unwrap();
            assert_eq!(component.request().unwrap().urls, vec![github]);
        }
    }

    #[test]
    fn component_name_follows_the_registry_contract() {
        // The registry only recognises this exact shape, so a rename here would
        // silently make every release asset undiscoverable.
        assert_eq!(
            component_filename(&platform("windows", "x86_64", "msvc")).expect("name"),
            "worldport-native-abi1-windows-x86_64-msvc.dll"
        );
        assert_eq!(
            component_filename(&platform("linux", "aarch64", "gnu")).expect("name"),
            "worldport-native-abi1-linux-aarch64-gnu.so"
        );
    }

    #[test]
    fn platforms_without_a_component_are_rejected_with_their_name() {
        let error = component_filename(&platform("macos", "x86_64", "gnu")).expect_err("unsupported");
        assert!(error.message.contains("macos"), "{}", error.message);
        let error = component_filename(&platform("windows", "riscv64", "msvc")).expect_err("arch");
        assert!(error.message.contains("riscv64"), "{}", error.message);
    }

    #[test]
    fn registry_document_must_match_the_plugin_identity() {
        let text = r#"{
            "schema_version": 1,
            "id": "worldport",
            "repository": "BMCBL/WorldPort",
            "status": "official",
            "version": "0.1.0",
            "native_assets": []
        }"#;
        let text = text.replace("0.1.0", env!("CARGO_PKG_VERSION"));
        // Parses, but there is no asset for this platform.
        assert!(registry_component(&text, &platform("windows", "x86_64", "msvc")).is_err());

        // A different plugin id in the same document must not be accepted.
        let impostor = text.replace(r#""id": "worldport""#, r#""id": "paperconnect""#);
        assert!(registry_component(&impostor, &platform("windows", "x86_64", "msvc")).is_err());
    }

    #[test]
    fn registry_rejects_a_download_host_the_contract_does_not_allow() {
        let sha = "a".repeat(64);
        let text = format!(
            r#"{{
                "schema_version": 1,
                "id": "worldport",
                "repository": "BMCBL/WorldPort",
                "status": "official",
                "version": "{version}",
                "native_assets": [{{
                    "abi_major": 1,
                    "os": "windows",
                    "arch": "x86_64",
                    "environment": "msvc",
                    "filename": "worldport-native-abi1-windows-x86_64-msvc.dll",
                    "size": 1024,
                    "sha256": "{sha}",
                    "sources": [{{"type": "github", "url": "https://evil.example/x.dll"}}]
                }}]
            }}"#,
            version = env!("CARGO_PKG_VERSION")
        );
        // The GitHub URL is not the release URL, and the required release URL is
        // absent, so the document is rejected rather than trusted.
        assert!(registry_component(&text, &platform("windows", "x86_64", "msvc")).is_err());
    }
}
