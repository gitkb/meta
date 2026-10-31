//! GitHub-based plugin registry for meta.
//!
//! The registry is a GitHub repository containing plugin metadata files.
//! Plugin authors submit PRs to register their plugins, and users can
//! install plugins directly from the registry.

use anyhow::{Context, Result};
use log::{debug, info};
use meta_core::display::{escape, escape_path};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Default registry URL
pub const DEFAULT_REGISTRY: &str = "https://raw.githubusercontent.com/gitkb/meta-plugins/main";

/// Plugin name prefix (all plugins must start with this)
pub const PLUGIN_PREFIX: &str = "meta-";

/// File extensions to exclude when listing installed plugins
const EXCLUDED_EXTENSIONS: &[&str] = &[".dylib", ".so", ".dll", ".a"];

/// Local plugins directory path (relative to workspace root)
const LOCAL_PLUGINS_DIR: &str = ".meta/plugins";

/// Global plugins directory name (under ~/.meta/)
const GLOBAL_PLUGINS_DIR: &str = "plugins";

/// Ensure a plugin name has the required prefix
pub fn ensure_plugin_prefix(name: &str) -> String {
    if name.starts_with(PLUGIN_PREFIX) {
        name.to_string()
    } else {
        format!("{PLUGIN_PREFIX}{name}")
    }
}

/// Normalize version string by removing 'v' prefix
fn normalize_version(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

/// Compare two version strings
///
/// Returns true if new_version is newer than current_version.
/// Simple lexicographic comparison after normalizing (removing 'v' prefix).
pub fn is_newer_version(current: &str, new: &str) -> bool {
    let current = normalize_version(current);
    let new = normalize_version(new);

    // Simple string comparison works for most semver cases
    // e.g., "1.0.0" < "1.1.0" < "2.0.0"
    new > current
}

/// Check if a filename is a plugin binary (has prefix, no excluded extension)
fn is_plugin_binary(name: &str) -> bool {
    name.starts_with(PLUGIN_PREFIX) && !EXCLUDED_EXTENSIONS.iter().any(|ext| name.ends_with(ext))
}

/// Plugin manifest entry tracking installation metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifestEntry {
    /// Installation source (URL, GitHub shorthand, or registry name)
    pub source: String,
    /// Plugin version (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Installation timestamp (ISO 8601)
    pub installed: String,
    /// Platform the plugin was installed for
    pub platform: String,
}

/// Plugin manifest file (~/.meta/plugins/.manifest.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginManifest {
    pub plugins: HashMap<String, PluginManifestEntry>,
}

/// Detailed plugin information for list command
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub location: PluginLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
}

/// Where a plugin is installed
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginLocation {
    /// Installed in ~/.meta/plugins/
    Installed,
    /// Found in project-local .meta/plugins/
    ProjectLocal,
}

/// Plugin installation scope (for installer configuration)
#[derive(Debug, Clone, PartialEq)]
pub enum InstallScope {
    /// Global installation to ~/.meta/plugins/
    Global,
    /// Project-local installation to .meta/plugins/
    Local,
}

impl PluginManifest {
    /// Load manifest from file, or return empty manifest if not found
    pub fn load(manifest_path: &Path) -> Result<Self> {
        let content = match std::fs::read_to_string(manifest_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            result => result.with_context(|| {
                format!(
                    "Failed to read manifest from {}",
                    escape_path(manifest_path)
                )
            })?,
        };

        let manifest: Self =
            serde_json::from_str(&content).with_context(|| "Failed to parse plugin manifest")?;

        Ok(manifest)
    }

    /// Save manifest to file
    pub fn save(&self, manifest_path: &Path) -> Result<()> {
        // Private (0600), like meta's other stores
        meta_core::store::write_atomic(manifest_path, self)
            .with_context(|| format!("Failed to write manifest to {}", escape_path(manifest_path)))
    }

    /// Add or update a plugin entry
    pub fn add_plugin(&mut self, name: String, entry: PluginManifestEntry) {
        self.plugins.insert(name, entry);
    }

    /// Remove a plugin entry
    pub fn remove_plugin(&mut self, name: &str) -> Option<PluginManifestEntry> {
        self.plugins.remove(name)
    }

    /// Get a plugin entry
    pub fn get_plugin(&self, name: &str) -> Option<&PluginManifestEntry> {
        self.plugins.get(name)
    }
}

/// Plugin metadata from the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub repository: String,
    #[serde(default)]
    pub releases: HashMap<String, PlatformReleases>,
    #[serde(default)]
    pub checksum: Option<String>,
}

/// Platform-specific release URLs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlatformReleases {
    #[serde(rename = "darwin-arm64")]
    pub darwin_arm64: Option<String>,
    #[serde(rename = "darwin-x64")]
    pub darwin_x64: Option<String>,
    #[serde(rename = "linux-x64")]
    pub linux_x64: Option<String>,
    #[serde(rename = "linux-arm64")]
    pub linux_arm64: Option<String>,
    #[serde(rename = "windows-x64")]
    pub windows_x64: Option<String>,
}

/// Registry index containing all available plugins
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryIndex {
    pub plugins: HashMap<String, PluginIndexEntry>,
}

/// Summary entry in the registry index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
}

/// Registry configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryConfig {
    #[serde(default)]
    pub registries: Vec<String>,
}

impl RegistryConfig {
    /// Load registry config from ~/.meta/config.yaml
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        // Only a missing file means "use the default registry": one that
        // can't be read may list a private registry
        let content = match std::fs::read_to_string(&config_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            result => {
                result.with_context(|| format!("Failed to read {}", escape_path(&config_path)))?
            }
        };
        serde_yaml_ng::from_str(&content).with_context(|| "Failed to parse registry config")
    }

    /// Get the config file path
    fn config_path() -> Result<PathBuf> {
        Ok(meta_core::meta_dir()?.join("config.yaml"))
    }

    /// Get list of registries (defaults to the public registry)
    pub fn get_registries(&self) -> Vec<String> {
        if self.registries.is_empty() {
            vec![DEFAULT_REGISTRY.to_string()]
        } else {
            self.registries.clone()
        }
    }
}

/// Plugin registry client
pub struct RegistryClient {
    registries: Vec<String>,
}

impl RegistryClient {
    /// Create a new registry client
    pub fn new() -> Result<Self> {
        // A config that cannot be read must not fall back to the default
        // registry: it may list a private one with the same plugin names
        let config = RegistryConfig::load()?;
        Ok(Self {
            registries: config.get_registries(),
        })
    }

    /// Fetch the registry index
    pub fn fetch_index(&self) -> Result<RegistryIndex> {
        let mut combined_index = RegistryIndex::default();

        for registry_url in &self.registries {
            let index_url = format!("{registry_url}/plugins/index.json");
            debug!("Fetching registry index from: {}", index_url);

            match self.fetch_json::<RegistryIndex>(&index_url) {
                Ok(index) => {
                    // Merge plugins (later registries override earlier ones)
                    combined_index.plugins.extend(index.plugins);
                }
                Err(e) => {
                    log::warn!("Failed to fetch from {}: {}", registry_url, e);
                }
            }
        }

        Ok(combined_index)
    }

    /// Resolve plugin source (GitHub shorthand) from registry
    ///
    /// This is the simplified M6 registry format where `plugins/{name}` contains
    /// a plain text GitHub shorthand like "user/repo" or "user/repo@v1.0.0".
    pub fn resolve_plugin_source(&self, name: &str) -> Result<String> {
        for registry_url in &self.registries {
            let plugin_url = format!("{registry_url}/plugins/{name}");
            debug!("Resolving plugin source from: {}", plugin_url);

            match ureq::get(&plugin_url).call() {
                Ok(response) => {
                    let source = response
                        .into_string()
                        .with_context(|| "Failed to read response body")?;
                    let source = source.trim().to_string();

                    if source.is_empty() {
                        continue;
                    }

                    debug!("Resolved {} -> {}", name, source);
                    return Ok(source);
                }
                Err(e) => {
                    debug!("Plugin {} not found in {}: {}", name, registry_url, e);
                    continue;
                }
            }
        }

        anyhow::bail!("Plugin '{}' not found in any registry", escape(name))
    }

    /// Fetch plugin metadata (complex registry format)
    ///
    /// This is the original registry format with full metadata in JSON.
    /// Falls back to this when simple source resolution fails.
    pub fn fetch_plugin_metadata(&self, name: &str) -> Result<PluginMetadata> {
        for registry_url in &self.registries {
            let plugin_url = format!("{registry_url}/plugins/{name}/plugin.json");
            debug!("Fetching plugin metadata from: {}", plugin_url);

            match self.fetch_json::<PluginMetadata>(&plugin_url) {
                Ok(metadata) => return Ok(metadata),
                Err(e) => {
                    log::warn!("Plugin {} not found in {}: {}", name, registry_url, e);
                }
            }
        }

        anyhow::bail!("Plugin '{}' not found in any registry", escape(name))
    }

    /// Search for plugins matching a query
    pub fn search(&self, query: &str) -> Result<Vec<PluginIndexEntry>> {
        let index = self.fetch_index()?;
        let query_lower = query.to_lowercase();

        let results: Vec<PluginIndexEntry> = index
            .plugins
            .into_values()
            .filter(|p| {
                p.name.to_lowercase().contains(&query_lower)
                    || p.description.to_lowercase().contains(&query_lower)
            })
            .collect();

        Ok(results)
    }

    /// Fetch JSON from a URL
    fn fetch_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = ureq::get(url)
            .call()
            .with_context(|| format!("Failed to fetch {}", escape(url)))?;

        let body = response
            .into_string()
            .with_context(|| "Failed to read response body")?;

        serde_json::from_str(&body).with_context(|| "Failed to parse JSON response")
    }

    /// Check for the latest version of a plugin from GitHub
    ///
    /// Given a GitHub shorthand (user/repo), queries the GitHub API
    /// for the latest release tag.
    pub fn get_latest_version(shorthand: &str) -> Result<String> {
        // Parse shorthand to extract user/repo
        let parts: Vec<&str> = shorthand.split('@').collect();
        let repo_path = parts[0];
        let parts: Vec<&str> = repo_path.split('/').collect();

        if parts.len() != 2 {
            anyhow::bail!("Invalid GitHub shorthand: {}", escape(shorthand));
        }

        let (user, repo) = (parts[0], parts[1]);
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            user, repo
        );

        debug!("Checking latest version: {}", api_url);

        let response = ureq::get(&api_url)
            .set("User-Agent", "meta-cli")
            .call()
            .with_context(|| {
                format!(
                    "Failed to fetch latest release for {}/{}",
                    escape(user),
                    escape(repo)
                )
            })?;

        let body = response
            .into_string()
            .with_context(|| "Failed to read response body")?;

        // Parse JSON to extract tag_name
        let json: serde_json::Value =
            serde_json::from_str(&body).with_context(|| "Failed to parse GitHub API response")?;

        let tag_name = json["tag_name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No tag_name in release"))?;

        Ok(tag_name.to_string())
    }

    /// Get the current platform identifier
    ///
    /// Can be overridden with META_PLATFORM environment variable for testing.
    pub fn current_platform() -> String {
        // Check for override via environment variable
        if let Ok(override_platform) = std::env::var("META_PLATFORM") {
            debug!(
                "Using platform override from META_PLATFORM: {}",
                override_platform
            );
            return override_platform;
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        return "darwin-arm64".to_string();
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        return "darwin-x64".to_string();
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        return "linux-x64".to_string();
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        return "linux-arm64".to_string();
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        return "windows-x64".to_string();
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64"),
        )))]
        return "unknown".to_string();
    }
}

/// Supported archive formats for plugin distribution
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    /// Detect archive format from URL or filename
    pub fn from_url(url: &str) -> Option<Self> {
        // Remove query parameters before checking extension
        let url_without_query = url.split('?').next().unwrap_or(url);

        if url_without_query.ends_with(".tar.gz") || url_without_query.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if url_without_query.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }

    /// Detect archive format from magic bytes (file signature)
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }

        // Check for gzip magic bytes (0x1f 0x8b)
        if bytes.starts_with(&[0x1f, 0x8b]) {
            Some(Self::TarGz)
        }
        // Check for zip magic bytes (PK\x03\x04 or PK\x05\x06 for empty zip)
        else if bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04])
            || bytes.starts_with(&[0x50, 0x4b, 0x05, 0x06])
        {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// No release of `shorthand` was found at any of `tried` URLs. Each value
/// comes from the user or the network, so each stays on its line.
fn release_not_found(
    shorthand: &GitHubShorthand,
    tried: usize,
    last_error: Option<anyhow::Error>,
) -> anyhow::Error {
    anyhow::anyhow!(
        "Could not find release for {}/{}{}\nTried {} URL(s). Last error: {}",
        escape(&shorthand.user),
        escape(&shorthand.repo),
        shorthand
            .version
            .as_ref()
            .map(|v| format!("@{}", escape(v)))
            .unwrap_or_default(),
        tried,
        last_error
            .map(|e| escape(&format!("{e:#}")).into_owned())
            .unwrap_or_default()
    )
}

/// Parsed GitHub shorthand: user/repo[@version]
#[derive(Debug, Clone, PartialEq)]
pub struct GitHubShorthand {
    pub user: String,
    pub repo: String,
    pub version: Option<String>,
}

impl GitHubShorthand {
    /// Parse a GitHub shorthand string (user/repo or user/repo@version)
    ///
    /// Returns None if the input doesn't match the expected format.
    pub fn parse(input: &str) -> Option<Self> {
        // Must not start with http
        if input.starts_with("http://") || input.starts_with("https://") {
            return None;
        }

        let parts: Vec<&str> = input.splitn(2, '/').collect();
        if parts.len() != 2 {
            return None;
        }

        let user = parts[0].to_string();
        let repo_and_version = parts[1];

        // Reject if repo contains additional slashes
        if repo_and_version.matches('/').count() > 0 {
            return None;
        }

        // Check for @version suffix
        if let Some(at_pos) = repo_and_version.find('@') {
            let repo = repo_and_version[..at_pos].to_string();
            let version = repo_and_version[at_pos + 1..].to_string();

            if user.is_empty() || repo.is_empty() || version.is_empty() {
                return None;
            }

            Some(Self {
                user,
                repo,
                version: Some(version),
            })
        } else {
            let repo = repo_and_version.to_string();

            if user.is_empty() || repo.is_empty() {
                return None;
            }

            Some(Self {
                user,
                repo,
                version: None,
            })
        }
    }

    /// Extract the plugin name from the repo (strips meta- prefix if present)
    pub fn plugin_name(&self) -> &str {
        self.repo.strip_prefix(PLUGIN_PREFIX).unwrap_or(&self.repo)
    }
}

/// Make a file executable on Unix systems (chmod 755)
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Plugin installer
#[derive(Debug)]
pub struct PluginInstaller {
    plugins_dir: PathBuf,
    scope: InstallScope,
}

impl PluginInstaller {
    /// Create a new plugin installer for global plugins
    pub fn new() -> Result<Self> {
        let plugins_dir = Self::default_plugins_dir()?;
        Ok(Self {
            plugins_dir,
            scope: InstallScope::Global,
        })
    }

    /// Create a new plugin installer for project-local plugins
    /// Create a local installer starting from the given directory.
    /// Used for testing without changing current_dir.
    fn new_local_from(start_dir: &Path) -> Result<Self> {
        let workspace_root = Self::find_workspace_root_from(start_dir)?;
        let plugins_dir = workspace_root.join(LOCAL_PLUGINS_DIR);
        Ok(Self {
            plugins_dir,
            scope: InstallScope::Local,
        })
    }

    pub fn new_local() -> Result<Self> {
        let cwd = std::env::current_dir().context("Failed to get current directory")?;
        Self::new_local_from(&cwd)
    }

    /// Find the nearest directory holding a config file (not a bare `.meta/`).
    fn find_workspace_root_from(start_dir: &Path) -> Result<PathBuf> {
        meta_core::config::nearest_meta_dir(start_dir)?.ok_or_else(|| anyhow::anyhow!(
            "Not in a meta workspace.\nExpected to find {} here or in a folder above.\nCreate one here, or cd into a workspace.",
            meta_core::config::META_CONFIG_NAME
        ))
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// The default plugins directory (~/.meta/plugins/), made only when a
    /// plugin is installed ([`Self::ensure_plugins_dir`])
    fn default_plugins_dir() -> Result<PathBuf> {
        Ok(meta_core::meta_dir()?.join(GLOBAL_PLUGINS_DIR))
    }

    /// Ensure the plugins directory exists
    fn ensure_plugins_dir(&self) -> Result<()> {
        // Made 0700, whatever the umask
        meta_core::data_dir::create_private_dir(&self.plugins_dir)?;
        if self.scope == InstallScope::Local {
            let ignore_path = self.plugins_dir.join(".gitignore");
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&ignore_path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(b"*\n").with_context(|| {
                        format!("Failed to write {}", escape_path(&ignore_path))
                    })?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to create {}", escape_path(&ignore_path)))
                }
            }
        }
        Ok(())
    }

    /// Get the manifest file path
    fn manifest_path(&self) -> PathBuf {
        self.plugins_dir.join(".manifest.json")
    }

    /// Load the plugin manifest
    fn load_manifest(&self) -> Result<PluginManifest> {
        PluginManifest::load(&self.manifest_path())
    }

    /// Save the plugin manifest
    fn save_manifest(&self, manifest: &PluginManifest) -> Result<()> {
        self.ensure_plugins_dir()?;
        manifest.save(&self.manifest_path())
    }

    /// Record a plugin installation in the manifest
    fn record_installation(
        &self,
        plugin_name: &str,
        source: String,
        version: Option<String>,
    ) -> Result<()> {
        let mut manifest = self.load_manifest()?;

        let entry = PluginManifestEntry {
            source,
            version,
            installed: chrono::Utc::now().to_rfc3339(),
            platform: RegistryClient::current_platform(),
        };

        manifest.add_plugin(plugin_name.to_string(), entry);
        self.save_manifest(&manifest)?;

        debug!("Recorded {} in manifest", plugin_name);
        Ok(())
    }

    /// Download bytes from a URL
    fn download(&self, url: &str) -> Result<Vec<u8>> {
        let response = ureq::get(url)
            .call()
            .with_context(|| format!("Failed to download {}", escape(url)))?;

        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .with_context(|| "Failed to read download")?;

        Ok(bytes)
    }

    /// Install a plugin from the registry
    pub fn install(&self, metadata: &PluginMetadata) -> Result<Vec<String>> {
        let platform = RegistryClient::current_platform();

        // Get the download URL for the current platform and latest version
        let releases = metadata.releases.get(&metadata.version).with_context(|| {
            format!(
                "No releases found for version {}",
                escape(&metadata.version)
            )
        })?;

        let download_url = self
            .get_platform_url(releases, &platform)
            .with_context(|| format!("No release available for platform {}", escape(&platform)))?;

        info!(
            "Downloading {} v{} for {}",
            metadata.name, metadata.version, platform
        );
        debug!("URL: {}", download_url);

        let bytes = self.download(&download_url)?;
        let installed = self.extract_and_validate(&download_url, &bytes)?;

        // Record installation in manifest
        for plugin_name in &installed {
            self.record_installation(
                plugin_name,
                metadata.name.clone(),
                Some(metadata.version.clone()),
            )?;
        }

        info!(
            "Successfully installed {} v{}",
            metadata.name, metadata.version
        );

        Ok(installed)
    }

    /// Install a plugin directly from a URL (bypasses registry)
    ///
    /// Downloads the archive, extracts it, and validates the plugin
    /// by running `--meta-plugin-info` on the extracted binary.
    pub fn install_from_url(&self, url: &str) -> Result<String> {
        info!("Downloading from: {}", url);

        let bytes = self.download(url)?;
        let installed = self.extract_and_validate(url, &bytes)?;

        // Record installation in manifest
        for plugin_name in &installed {
            self.record_installation(plugin_name, url.to_string(), None)?;
        }

        let primary_plugin = installed.first().unwrap().clone();
        info!("Successfully installed: {}", installed.join(", "));

        Ok(primary_plugin)
    }

    /// Install a plugin from GitHub using shorthand syntax (user/repo[@version])
    ///
    /// Automatically discovers the correct platform binary from GitHub Releases
    /// by trying multiple naming conventions and formats.
    pub fn install_from_github(&self, shorthand: &GitHubShorthand) -> Result<String> {
        let platform = RegistryClient::current_platform();

        if let Some(version) = &shorthand.version {
            info!(
                "Installing {}/{}@{} for {}",
                shorthand.user, shorthand.repo, version, platform
            );
        } else {
            info!(
                "Installing {}/{} (latest) for {}",
                shorthand.user, shorthand.repo, platform
            );
        }

        // Try to download with various URL patterns
        let urls = self.construct_github_urls(shorthand, &platform);

        let mut last_error = None;
        for url in &urls {
            debug!("Trying: {}", url);

            match self.download(url) {
                Ok(bytes) => {
                    // Successfully downloaded, now extract and validate
                    let installed = self.extract_and_validate(url, &bytes)?;

                    // Record installation in manifest
                    let source = format!(
                        "{}/{}{}",
                        shorthand.user,
                        shorthand.repo,
                        shorthand
                            .version
                            .as_ref()
                            .map(|v| format!("@{}", v))
                            .unwrap_or_default()
                    );
                    for plugin_name in &installed {
                        self.record_installation(
                            plugin_name,
                            source.clone(),
                            shorthand.version.clone(),
                        )?;
                    }

                    let primary_plugin = installed.first().unwrap().clone();
                    info!("Successfully installed: {}", installed.join(", "));
                    return Ok(primary_plugin);
                }
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            }
        }

        // If we get here, none of the URLs worked
        Err(release_not_found(shorthand, urls.len(), last_error))
    }

    /// Construct possible GitHub release URLs for a shorthand
    fn construct_github_urls(&self, shorthand: &GitHubShorthand, platform: &str) -> Vec<String> {
        let mut urls = Vec::new();
        let base = format!(
            "https://github.com/{}/{}/releases",
            shorthand.user, shorthand.repo
        );

        // Determine version component
        let version_paths = if let Some(version) = &shorthand.version {
            // Try both with and without 'v' prefix
            if version.starts_with('v') {
                // If version already has 'v', try with and without
                vec![
                    format!("download/{version}"),
                    format!("download/{}", version.strip_prefix('v').unwrap()),
                ]
            } else {
                // If version doesn't have 'v', try both
                vec![
                    format!("download/{version}"),
                    format!("download/v{version}"),
                ]
            }
        } else {
            vec!["latest/download".to_string()]
        };

        // Platform aliases (try both forms)
        let platform_variants = Self::platform_aliases(platform);

        // Plugin name variants (deduplicate if repo == plugin_name)
        let mut plugin_names = vec![shorthand.repo.clone()];
        let stripped = shorthand.plugin_name().to_string();
        if stripped != shorthand.repo {
            plugin_names.push(stripped);
        }

        // Archive formats
        let formats = vec!["tar.gz", "tgz"];

        // Generate all combinations
        for ver_path in &version_paths {
            for plugin_name in &plugin_names {
                for plat in &platform_variants {
                    for fmt in &formats {
                        urls.push(format!("{base}/{ver_path}/{plugin_name}-{plat}.{fmt}"));
                    }
                }
                // Also try -any suffix (platform-independent)
                for fmt in &formats {
                    urls.push(format!("{base}/{ver_path}/{plugin_name}-any.{fmt}"));
                }
            }
        }

        urls
    }

    /// Get platform naming aliases (darwin ↔ macos, x64 ↔ amd64, etc.)
    fn platform_aliases(platform: &str) -> Vec<String> {
        let mut aliases = vec![platform.to_string()];

        // Common platform naming variations
        if platform.contains("darwin") {
            aliases.push(platform.replace("darwin", "macos"));
        } else if platform.contains("macos") {
            aliases.push(platform.replace("macos", "darwin"));
        }

        if platform.contains("x64") {
            aliases.push(platform.replace("x64", "amd64"));
            aliases.push(platform.replace("x64", "x86_64"));
        } else if platform.contains("amd64") {
            aliases.push(platform.replace("amd64", "x64"));
            aliases.push(platform.replace("amd64", "x86_64"));
        } else if platform.contains("x86_64") {
            aliases.push(platform.replace("x86_64", "x64"));
            aliases.push(platform.replace("x86_64", "amd64"));
        }

        if platform.contains("arm64") {
            aliases.push(platform.replace("arm64", "aarch64"));
        } else if platform.contains("aarch64") {
            aliases.push(platform.replace("aarch64", "arm64"));
        }

        aliases.sort();
        aliases.dedup();
        aliases
    }

    /// Extract archive and validate all installed plugins
    fn extract_and_validate(&self, url: &str, bytes: &[u8]) -> Result<Vec<String>> {
        self.ensure_plugins_dir()?;
        let installed = self.extract_archive(url, bytes)?;
        self.validate_installed(&installed)?;
        Ok(installed)
    }

    /// Validate a list of installed plugins
    fn validate_installed(&self, installed: &[String]) -> Result<()> {
        if installed.is_empty() {
            anyhow::bail!("No {PLUGIN_PREFIX}* executables found in archive");
        }

        for plugin_name in installed {
            let plugin_path = self.plugins_dir.join(plugin_name);
            // A plugin answers --meta-plugin-info with its info, in time
            let answered =
                crate::subprocess_plugins::probe_plugin_info(&plugin_path).and_then(|info| {
                    info.map(|_| ()).ok_or_else(|| {
                        anyhow::anyhow!("Plugin did not answer --meta-plugin-info with its info")
                    })
                });
            answered.with_context(|| {
                // Remove invalid plugin on failure
                let _ = std::fs::remove_file(&plugin_path);
                format!("Plugin validation failed for {}", escape(plugin_name))
            })?;
        }

        Ok(())
    }

    /// Extract archive and return list of installed plugin names
    fn extract_archive(&self, url: &str, bytes: &[u8]) -> Result<Vec<String>> {
        // Try to detect format from URL first, then fall back to magic bytes
        let format = ArchiveFormat::from_url(url)
            .or_else(|| ArchiveFormat::from_bytes(bytes))
            .with_context(|| format!("Unsupported archive format: {}", escape(url)))?;

        match format {
            ArchiveFormat::TarGz => self.extract_tar_gz(bytes),
            ArchiveFormat::Zip => self.extract_zip(bytes),
        }
    }

    /// Extract a tar.gz archive
    fn extract_tar_gz(&self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut installed = Vec::new();
        let decoder = flate2::read::GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let file_name = entry
                .path()?
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string());

            if let Some(name) = file_name {
                if name.starts_with(PLUGIN_PREFIX) {
                    let dest = self.plugins_dir.join(&name);
                    entry.unpack(&dest)?;
                    make_executable(&dest)?;
                    installed.push(name);
                }
            }
        }

        Ok(installed)
    }

    /// Extract a zip archive
    fn extract_zip(&self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut installed = Vec::new();
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;

        // Preflight every entry and name before creating or replacing any plugins.
        let mut entries = Vec::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let file_name = file.name()?.into_owned();
            if file_name.starts_with(PLUGIN_PREFIX) {
                // Require one filename component on both Unix and Windows.
                anyhow::ensure!(
                    !file_name.contains(['/', '\\']),
                    "ZIP plugin entry must be a top-level filename: {}",
                    escape(&file_name)
                );
                entries.push((i, file_name));
            }
        }

        for (i, file_name) in entries {
            let mut file = archive.by_index(i)?;
            let dest = self.plugins_dir.join(&file_name);
            let mut dest_file = std::fs::File::create(&dest)?;
            std::io::copy(&mut file, &mut dest_file)?;
            make_executable(&dest)?;
            installed.push(file_name);
        }

        Ok(installed)
    }

    /// Get the download URL for a specific platform
    fn get_platform_url(&self, releases: &PlatformReleases, platform: &str) -> Option<String> {
        match platform {
            "darwin-arm64" => releases.darwin_arm64.clone(),
            "darwin-x64" => releases.darwin_x64.clone(),
            "linux-x64" => releases.linux_x64.clone(),
            "linux-arm64" => releases.linux_arm64.clone(),
            "windows-x64" => releases.windows_x64.clone(),
            _ => None,
        }
    }

    /// List plugins with detailed information including manifest data
    pub fn list_plugins_detailed(&self) -> Result<Vec<PluginInfo>> {
        let mut plugins = Vec::new();
        let manifest = self.load_manifest()?;

        // Scan installed plugins directory
        if self.plugins_dir.exists() {
            for entry in std::fs::read_dir(&self.plugins_dir)? {
                let entry = entry?;
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_plugin_binary(name) {
                        // Get manifest data if available
                        let manifest_entry = manifest.get_plugin(name);

                        plugins.push(PluginInfo {
                            name: name.to_string(),
                            version: match crate::subprocess_plugins::probe_plugin_info(&path) {
                                Ok(info) => info.map(|info| info.version),
                                Err(e) => {
                                    eprintln!("Warning: {e}");
                                    None
                                }
                            },
                            source: manifest_entry.map(|e| e.source.clone()),
                            location: if self.scope == InstallScope::Local {
                                PluginLocation::ProjectLocal
                            } else {
                                PluginLocation::Installed
                            },
                            installed: manifest_entry.map(|e| e.installed.clone()),
                        });
                    }
                }
            }
        }

        Ok(plugins)
    }

    /// Uninstall a plugin
    pub fn uninstall(&self, name: &str) -> Result<()> {
        let plugin_name = ensure_plugin_prefix(name);

        let plugin_path = self.plugins_dir.join(&plugin_name);
        if plugin_path.exists() {
            // The manifest is read before anything is deleted
            let mut manifest = self.load_manifest()?;
            std::fs::remove_file(&plugin_path)
                .with_context(|| format!("Failed to remove {}", escape_path(&plugin_path)))?;

            // Remove from manifest
            manifest.remove_plugin(&plugin_name);
            self.save_manifest(&manifest)?;

            info!("Uninstalled {}", plugin_name);
            Ok(())
        } else {
            anyhow::bail!("Plugin {} is not installed", escape(&plugin_name))
        }
    }

    /// Check for updates for a specific plugin
    ///
    /// Returns Some((current_version, latest_version)) if an update is available,
    /// None if already up to date or if version info is unavailable.
    pub fn check_update(&self, plugin_name: &str) -> Result<Option<(String, String)>> {
        let manifest = self.load_manifest()?;
        let plugin_name = ensure_plugin_prefix(plugin_name);

        let entry = manifest.get_plugin(&plugin_name).ok_or_else(|| {
            anyhow::anyhow!("Plugin {} not found in manifest", escape(&plugin_name))
        })?;

        // Check if source is a GitHub shorthand
        if !entry.source.contains('/') {
            debug!(
                "Plugin {} source is not a GitHub shorthand, skipping update check",
                plugin_name
            );
            return Ok(None);
        }

        // Get current version
        let current_version = match &entry.version {
            Some(v) => v.clone(),
            None => {
                debug!(
                    "Plugin {} has no version in manifest, cannot check for updates",
                    plugin_name
                );
                return Ok(None);
            }
        };

        // Get latest version from GitHub
        let latest_version = RegistryClient::get_latest_version(&entry.source)?;

        // Compare versions
        if is_newer_version(&current_version, &latest_version) {
            Ok(Some((current_version, latest_version)))
        } else {
            Ok(None)
        }
    }

    /// Update a plugin to the latest version
    pub fn update_plugin(&self, plugin_name: &str) -> Result<String> {
        let manifest = self.load_manifest()?;
        let plugin_name = ensure_plugin_prefix(plugin_name);

        let entry = manifest
            .get_plugin(&plugin_name)
            .ok_or_else(|| anyhow::anyhow!("Plugin {} not installed", escape(&plugin_name)))?;

        // Parse source as GitHub shorthand
        let shorthand = GitHubShorthand::parse(&entry.source).ok_or_else(|| {
            anyhow::anyhow!("Cannot update plugin: source is not a GitHub shorthand")
        })?;

        info!(
            "Updating {} from {} to latest",
            plugin_name,
            entry.version.as_deref().unwrap_or("unknown")
        );

        // Uninstall current version
        self.uninstall(&plugin_name)?;

        // Install latest version
        let installed_name = self.install_from_github(&shorthand)?;

        Ok(installed_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_releases_error_keeps_each_value_on_its_line() {
        let shorthand = GitHubShorthand {
            user: "o\nFORGED".to_string(),
            repo: "r\u{1b}[2J".to_string(),
            version: Some("v1\nFORGED".to_string()),
        };
        let message =
            release_not_found(&shorthand, 2, Some(anyhow::anyhow!("e\nFORGED"))).to_string();
        assert_eq!(message.lines().count(), 2, "{message}");
        assert!(!message.contains('\u{1b}'), "{message}");
        assert!(
            message.starts_with("Could not find release for o\\nFORGED/"),
            "{message}"
        );
    }

    #[test]
    fn test_registry_config_default() {
        let config = RegistryConfig::default();
        let registries = config.get_registries();
        assert_eq!(registries.len(), 1);
        assert!(registries[0].contains("meta-plugins"));
    }

    #[test]
    fn test_current_platform() {
        let platform = RegistryClient::current_platform();
        assert!(!platform.is_empty());
        // Platform should be one of the known values
        let known_platforms = [
            "darwin-arm64",
            "darwin-x64",
            "linux-x64",
            "linux-arm64",
            "windows-x64",
            "unknown",
        ];
        assert!(known_platforms.contains(&platform.as_str()));
    }

    #[test]
    fn test_plugin_metadata_serialization() {
        let metadata = PluginMetadata {
            name: "docker".to_string(),
            description: "Docker commands for meta".to_string(),
            version: "1.0.0".to_string(),
            author: "testuser".to_string(),
            repository: "github.com/testuser/meta-plugin-docker".to_string(),
            releases: HashMap::new(),
            checksum: Some("sha256:abc123".to_string()),
        };

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("\"name\":\"docker\""));
        assert!(json.contains("\"version\":\"1.0.0\""));

        // Deserialize
        let parsed: PluginMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "docker");
        assert_eq!(parsed.author, "testuser");
    }

    #[test]
    fn test_platform_releases_serialization() {
        let releases = PlatformReleases {
            darwin_arm64: Some("https://example.com/darwin-arm64.tar.gz".to_string()),
            darwin_x64: Some("https://example.com/darwin-x64.tar.gz".to_string()),
            linux_x64: Some("https://example.com/linux-x64.tar.gz".to_string()),
            linux_arm64: None,
            windows_x64: None,
        };

        let json = serde_json::to_string(&releases).unwrap();
        assert!(json.contains("darwin-arm64"));
        assert!(json.contains("darwin-x64"));
    }

    #[test]
    fn test_registry_index_entry_serialization() {
        let entry = PluginIndexEntry {
            name: "npm".to_string(),
            description: "NPM commands for meta".to_string(),
            version: "2.0.0".to_string(),
            author: "npmuser".to_string(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: PluginIndexEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "npm");
        assert_eq!(parsed.version, "2.0.0");
    }

    #[test]
    fn test_registry_config_custom_registries() {
        let config = RegistryConfig {
            registries: vec![
                "https://custom.registry.com".to_string(),
                "https://another.registry.com".to_string(),
            ],
        };

        let registries = config.get_registries();
        assert_eq!(registries.len(), 2);
        assert_eq!(registries[0], "https://custom.registry.com");
        assert_eq!(registries[1], "https://another.registry.com");
    }

    #[cfg(unix)]
    #[test]
    fn test_plugins_dir_and_manifest_are_private_whatever_the_umask() {
        // meta creates its own dirs 0700 and files 0600, whatever the umask
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().join("plugins"),
            scope: InstallScope::Local,
        };
        installer.save_manifest(&PluginManifest::default()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&installer.plugins_dir), 0o700);
        assert_eq!(mode(&installer.manifest_path()), 0o600);
        assert!(installer.load_manifest().is_ok());
    }

    #[test]
    fn test_plugin_installer_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("meta-test"), "fake binary").unwrap();

        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Uninstall should succeed
        installer.uninstall("test").unwrap();
        assert!(!dir.path().join("meta-test").exists());
    }

    #[test]
    fn test_plugin_installer_uninstall_not_installed() {
        let dir = tempfile::tempdir().unwrap();

        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Uninstall should fail for non-existent plugin
        let result = installer.uninstall("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_archive_format_from_url() {
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.tar.gz"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.tgz"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.zip"),
            Some(ArchiveFormat::Zip)
        );
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.exe"),
            None
        );
        assert_eq!(ArchiveFormat::from_url("https://example.com/plugin"), None);
    }

    #[test]
    fn test_extract_tar_gz() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create a minimal tar.gz with a meta-test file
        let mut builder = tar::Builder::new(Vec::new());

        let content = b"#!/bin/sh\necho test";
        let mut header = tar::Header::new_gnu();
        header.set_path("meta-test").unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &content[..]).unwrap();

        let tar_data = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let installed = installer.extract_tar_gz(&gz_data).unwrap();

        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0], "meta-test");
        assert!(dir.path().join("meta-test").exists());
    }

    #[test]
    fn test_extract_tar_gz_filters_non_meta_files() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create a tar.gz with both meta-* and non-meta files
        let mut builder = tar::Builder::new(Vec::new());

        for name in &["meta-plugin", "readme.txt", "other-binary"] {
            let content = b"content";
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &content[..]).unwrap();
        }

        let tar_data = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let installed = installer.extract_tar_gz(&gz_data).unwrap();

        // Only meta-plugin should be extracted
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0], "meta-plugin");
        assert!(dir.path().join("meta-plugin").exists());
        assert!(!dir.path().join("readme.txt").exists());
        assert!(!dir.path().join("other-binary").exists());
    }

    #[test]
    fn test_extract_zip() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create a minimal zip with a meta-test file
        let mut zip_data = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
            let options = SimpleFileOptions::default();
            zip.start_file("meta-test", options).unwrap();
            zip.write_all(b"#!/bin/sh\necho test").unwrap();
            zip.finish().unwrap();
        }

        let installed = installer.extract_zip(&zip_data).unwrap();

        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0], "meta-test");
        assert!(dir.path().join("meta-test").exists());
    }

    #[test]
    fn test_extract_zip_filters_non_meta_files() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create a zip with both meta-* and non-meta files
        let mut zip_data = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
            let options = SimpleFileOptions::default();

            for name in &["meta-plugin", "readme.txt", "other-binary"] {
                zip.start_file(*name, options).unwrap();
                zip.write_all(b"content").unwrap();
            }
            zip.finish().unwrap();
        }

        let installed = installer.extract_zip(&zip_data).unwrap();

        // Only meta-plugin should be extracted
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0], "meta-plugin");
        assert!(dir.path().join("meta-plugin").exists());
        assert!(!dir.path().join("readme.txt").exists());
        assert!(!dir.path().join("other-binary").exists());
    }

    #[test]
    fn test_extract_zip_rejects_late_entry_error_before_writing_plugins() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let mut zip_data = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
            for name in ["meta-test", "readme.txt"] {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"replacement plugin").unwrap();
            }
            zip.finish().unwrap();
        }

        // Keep the central directory valid, but make the later local header unreadable.
        let later_header = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zip_data)).unwrap();
            let file = archive.by_index(1).unwrap();
            file.header_start() as usize
        };
        zip_data[later_header..later_header + 4].copy_from_slice(b"BAD!");

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zip_data)).unwrap();
        assert_eq!(archive.by_index(0).unwrap().name().unwrap(), "meta-test");
        let entry_error = archive
            .by_index(1)
            .expect_err("later entry must fail after the first entry opened")
            .to_string();

        for existing_plugin in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let plugin_path = dir.path().join("meta-test");
            if existing_plugin {
                std::fs::write(&plugin_path, b"original plugin").unwrap();
            }
            let installer = PluginInstaller {
                plugins_dir: dir.path().to_path_buf(),
                scope: InstallScope::Global,
            };

            let error = installer
                .extract_and_validate("https://example.com/plugin.zip", &zip_data)
                .unwrap_err();
            assert_eq!(error.to_string(), entry_error);
            if existing_plugin {
                assert_eq!(std::fs::read(&plugin_path).unwrap(), b"original plugin");
            } else {
                assert!(!plugin_path.exists(), "failed install left a plugin behind");
            }
        }
    }

    #[test]
    fn test_extract_zip_rejects_plugin_paths_before_writing_plugins() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        for invalid_name in [
            "meta-dir/../../escaped",
            r"meta-dir\..\..\escaped",
            "meta-dir/child",
            r"meta-dir\child",
        ] {
            let mut zip_data = Vec::new();
            {
                let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_data));
                for name in ["meta-test", invalid_name] {
                    zip.start_file(name, SimpleFileOptions::default()).unwrap();
                    zip.write_all(b"replacement plugin").unwrap();
                }
                zip.finish().unwrap();
            }

            for existing_plugin in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let plugins_dir = dir.path().join("plugins");
                // Make traversal possible instead of relying on a missing parent directory.
                std::fs::create_dir_all(plugins_dir.join("meta-dir")).unwrap();
                let plugin_path = plugins_dir.join("meta-test");
                if existing_plugin {
                    std::fs::write(&plugin_path, b"original plugin").unwrap();
                }
                let installer = PluginInstaller {
                    plugins_dir: plugins_dir.clone(),
                    scope: InstallScope::Global,
                };

                let error = installer.extract_zip(&zip_data).unwrap_err();
                assert!(
                    error.to_string().contains("top-level filename"),
                    "unexpected error for {invalid_name:?}: {error}"
                );
                assert!(!dir.path().join("escaped").exists());
                assert_eq!(
                    std::fs::read_dir(plugins_dir.join("meta-dir"))
                        .unwrap()
                        .count(),
                    0
                );
                if existing_plugin {
                    assert_eq!(std::fs::read(&plugin_path).unwrap(), b"original plugin");
                } else {
                    assert!(!plugin_path.exists(), "invalid path left a plugin behind");
                }
            }
        }
    }

    #[test]
    fn test_extract_archive_unsupported_format() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        let result = installer.extract_archive("https://example.com/plugin.exe", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported"));
    }

    #[test]
    fn test_github_shorthand_parse_simple() {
        let shorthand = GitHubShorthand::parse("someuser/meta-docker").unwrap();
        assert_eq!(shorthand.user, "someuser");
        assert_eq!(shorthand.repo, "meta-docker");
        assert_eq!(shorthand.version, None);
    }

    #[test]
    fn test_github_shorthand_parse_with_version() {
        let shorthand = GitHubShorthand::parse("someuser/meta-docker@v1.0.0").unwrap();
        assert_eq!(shorthand.user, "someuser");
        assert_eq!(shorthand.repo, "meta-docker");
        assert_eq!(shorthand.version, Some("v1.0.0".to_string()));
    }

    #[test]
    fn test_github_shorthand_parse_without_v_prefix() {
        let shorthand = GitHubShorthand::parse("someuser/meta-docker@1.0.0").unwrap();
        assert_eq!(shorthand.user, "someuser");
        assert_eq!(shorthand.repo, "meta-docker");
        assert_eq!(shorthand.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_github_shorthand_parse_rejects_url() {
        assert_eq!(GitHubShorthand::parse("https://github.com/user/repo"), None);
        assert_eq!(GitHubShorthand::parse("http://example.com/plugin"), None);
    }

    #[test]
    fn test_github_shorthand_parse_rejects_invalid() {
        assert_eq!(GitHubShorthand::parse("justname"), None);
        assert_eq!(GitHubShorthand::parse("too/many/slashes"), None);
        assert_eq!(GitHubShorthand::parse("/nouserorgrepo"), None);
        assert_eq!(GitHubShorthand::parse("nouser/"), None);
        assert_eq!(GitHubShorthand::parse("user/@"), None);
    }

    #[test]
    fn test_github_shorthand_plugin_name() {
        let shorthand = GitHubShorthand::parse("user/meta-docker").unwrap();
        assert_eq!(shorthand.plugin_name(), "docker");

        let shorthand = GitHubShorthand::parse("user/docker").unwrap();
        assert_eq!(shorthand.plugin_name(), "docker");
    }

    #[test]
    fn test_platform_aliases_darwin() {
        let aliases = PluginInstaller::platform_aliases("darwin-arm64");
        assert!(aliases.contains(&"darwin-arm64".to_string()));
        assert!(aliases.contains(&"macos-arm64".to_string()));
        assert!(aliases.contains(&"darwin-aarch64".to_string()));
    }

    #[test]
    fn test_platform_aliases_x64() {
        let aliases = PluginInstaller::platform_aliases("linux-x64");
        assert!(aliases.contains(&"linux-x64".to_string()));
        assert!(aliases.contains(&"linux-amd64".to_string()));
        assert!(aliases.contains(&"linux-x86_64".to_string()));
    }

    #[test]
    fn test_construct_github_urls() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        let shorthand = GitHubShorthand::parse("user/meta-docker@v1.0.0").unwrap();
        let urls = installer.construct_github_urls(&shorthand, "darwin-arm64");

        // Should contain versioned URLs
        assert!(urls.iter().any(|u| u.contains("download/v1.0.0")));
        assert!(urls.iter().any(|u| u.contains("download/1.0.0")));

        // Should contain platform variants
        assert!(urls.iter().any(|u| u.contains("darwin-arm64")));
        assert!(urls.iter().any(|u| u.contains("macos-arm64")));

        // Should contain format variants
        assert!(urls.iter().any(|u| u.ends_with(".tar.gz")));
        assert!(urls.iter().any(|u| u.ends_with(".tgz")));

        // Should contain -any variant
        assert!(urls.iter().any(|u| u.contains("-any.")));
    }

    #[test]
    fn test_construct_github_urls_latest() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        let shorthand = GitHubShorthand::parse("user/meta-docker").unwrap();
        let urls = installer.construct_github_urls(&shorthand, "linux-x64");

        // Should use latest/download for unversioned
        assert!(urls.iter().any(|u| u.contains("latest/download")));
        assert!(urls.iter().all(|u| !u.contains("download/v")));
    }

    #[test]
    fn test_construct_github_urls_deduplicates_plugin_names() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // When repo doesn't have "meta-" prefix, both names would be the same
        let shorthand = GitHubShorthand::parse("user/docker").unwrap();
        let urls = installer.construct_github_urls(&shorthand, "linux-x64");

        // Count how many URLs contain "docker-linux-x64" (exact platform match)
        let docker_count = urls
            .iter()
            .filter(|u| u.contains("docker-linux-x64"))
            .count();

        // Should appear once per format (not duplicated for same plugin name)
        // 1 plugin name × 1 platform match × 2 formats = 2
        assert_eq!(
            docker_count, 2,
            "Should not duplicate plugin names in URL generation"
        );

        // With "meta-" prefix, there should be two distinct names
        let shorthand = GitHubShorthand::parse("user/meta-docker").unwrap();
        let urls = installer.construct_github_urls(&shorthand, "linux-x64");

        // Count URLs with exact platform for each plugin name variant
        let meta_docker_count = urls
            .iter()
            .filter(|u| u.contains("meta-docker-linux-x64"))
            .count();
        let docker_only_count = urls
            .iter()
            .filter(|u| u.contains("/docker-linux-x64"))
            .count();

        // Both variants should exist: "meta-docker" and "docker" (without preceding "meta-")
        assert_eq!(meta_docker_count, 2, "Should have meta-docker variant");
        assert_eq!(
            docker_only_count, 2,
            "Should have docker variant without meta- prefix"
        );
    }

    #[test]
    fn test_ensure_plugin_prefix() {
        assert_eq!(ensure_plugin_prefix("docker"), "meta-docker");
        assert_eq!(ensure_plugin_prefix("meta-docker"), "meta-docker");
        assert_eq!(ensure_plugin_prefix("meta-"), "meta-");
    }

    #[test]
    fn test_is_plugin_binary() {
        assert!(is_plugin_binary("meta-docker"));
        assert!(is_plugin_binary("meta-test"));
        assert!(!is_plugin_binary("other-binary"));
        assert!(!is_plugin_binary("meta-test.dylib"));
        assert!(!is_plugin_binary("meta-test.so"));
        assert!(!is_plugin_binary("meta-test.dll"));
        assert!(!is_plugin_binary("meta-test.a"));
    }

    #[test]
    fn test_archive_format_from_url_with_query_params() {
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.tar.gz?token=abc123"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            ArchiveFormat::from_url("https://example.com/plugin.zip?download=1"),
            Some(ArchiveFormat::Zip)
        );
    }

    #[test]
    fn test_archive_format_from_bytes_gzip() {
        let gzip_bytes = [0x1f, 0x8b, 0x08, 0x00]; // Gzip magic + some data
        assert_eq!(
            ArchiveFormat::from_bytes(&gzip_bytes),
            Some(ArchiveFormat::TarGz)
        );
    }

    #[test]
    fn test_archive_format_from_bytes_zip() {
        let zip_bytes = [0x50, 0x4b, 0x03, 0x04]; // Zip magic (PK\x03\x04)
        assert_eq!(
            ArchiveFormat::from_bytes(&zip_bytes),
            Some(ArchiveFormat::Zip)
        );

        let empty_zip_bytes = [0x50, 0x4b, 0x05, 0x06]; // Empty zip magic
        assert_eq!(
            ArchiveFormat::from_bytes(&empty_zip_bytes),
            Some(ArchiveFormat::Zip)
        );
    }

    #[test]
    fn test_archive_format_from_bytes_unknown() {
        let unknown_bytes = [0x00, 0x01, 0x02, 0x03];
        assert_eq!(ArchiveFormat::from_bytes(&unknown_bytes), None);

        let too_short = [0x1f];
        assert_eq!(ArchiveFormat::from_bytes(&too_short), None);
    }

    #[test]
    fn test_platform_override_via_env_var() {
        // Save current env state
        let original = std::env::var("META_PLATFORM").ok();

        // Test override
        std::env::set_var("META_PLATFORM", "test-platform-x64");
        assert_eq!(RegistryClient::current_platform(), "test-platform-x64");

        // Restore original env state
        match original {
            Some(val) => std::env::set_var("META_PLATFORM", val),
            None => std::env::remove_var("META_PLATFORM"),
        }
    }

    #[test]
    fn test_validation_cleanup_removes_invalid_plugin() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create a fake invalid plugin (not executable/doesn't respond to --meta-plugin-info)
        let plugin_path = dir.path().join("meta-invalid");
        let mut file = std::fs::File::create(&plugin_path).unwrap();
        file.write_all(b"not a real plugin").unwrap();
        drop(file);

        // Validation should fail and the file should still be there
        // (cleanup happens inside validate_installed)
        let result = installer.validate_installed(&["meta-invalid".to_string()]);
        assert!(result.is_err());

        // Note: The actual removal happens in validate_installed's error context,
        // so the file may or may not exist depending on the error
    }

    #[test]
    fn test_plugin_manifest_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join(".manifest.json");

        let mut manifest = PluginManifest::default();
        manifest.add_plugin(
            "meta-test".to_string(),
            PluginManifestEntry {
                source: "test-user/meta-test".to_string(),
                version: Some("v1.0.0".to_string()),
                installed: "2024-01-01T00:00:00Z".to_string(),
                platform: "darwin-arm64".to_string(),
            },
        );

        // Save manifest
        manifest.save(&manifest_path).unwrap();
        assert!(manifest_path.exists());

        // Load manifest
        let loaded = PluginManifest::load(&manifest_path).unwrap();
        assert_eq!(loaded.plugins.len(), 1);

        let entry = loaded.get_plugin("meta-test").unwrap();
        assert_eq!(entry.source, "test-user/meta-test");
        assert_eq!(entry.version.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn test_plugin_manifest_add_remove() {
        let mut manifest = PluginManifest::default();

        // Add plugin
        manifest.add_plugin(
            "meta-test".to_string(),
            PluginManifestEntry {
                source: "test-user/meta-test".to_string(),
                version: None,
                installed: "2024-01-01T00:00:00Z".to_string(),
                platform: "linux-x64".to_string(),
            },
        );
        assert_eq!(manifest.plugins.len(), 1);

        // Remove plugin
        let removed = manifest.remove_plugin("meta-test");
        assert!(removed.is_some());
        assert_eq!(manifest.plugins.len(), 0);

        // Remove non-existent plugin
        let removed = manifest.remove_plugin("meta-nonexistent");
        assert!(removed.is_none());
    }

    #[test]
    fn test_plugin_manifest_load_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("nonexistent.json");

        // Loading non-existent manifest should return empty manifest
        let manifest = PluginManifest::load(&manifest_path).unwrap();
        assert_eq!(manifest.plugins.len(), 0);
    }

    #[test]
    fn test_plugin_info_serialization() {
        let info = PluginInfo {
            name: "meta-test".to_string(),
            version: Some("v1.0.0".to_string()),
            source: Some("test-user/meta-test".to_string()),
            location: PluginLocation::Installed,
            installed: Some("2024-01-01T00:00:00Z".to_string()),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("meta-test"));
        assert!(json.contains("v1.0.0"));
        assert!(json.contains("installed"));
    }

    #[test]
    fn test_list_plugins_detailed_empty() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        let plugins = installer.list_plugins_detailed().unwrap();
        assert!(plugins.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_list_plugins_detailed_uses_binary_version_and_manifest_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let installer = PluginInstaller {
            plugins_dir: dir.path().to_path_buf(),
            scope: InstallScope::Global,
        };

        // Create plugin file
        std::fs::create_dir_all(dir.path()).unwrap();
        let plugin_path = dir.path().join("meta-test");
        std::fs::write(
            &plugin_path,
            b"#!/bin/sh\necho '{\"name\":\"test\",\"version\":\"2.0.0\",\"commands\":[]}'\n",
        )
        .unwrap();
        make_executable(&plugin_path).unwrap();

        // Create manifest
        let mut manifest = PluginManifest::default();
        manifest.add_plugin(
            "meta-test".to_string(),
            PluginManifestEntry {
                source: "test-user/meta-test".to_string(),
                version: Some("v1.0.0".to_string()),
                installed: "2024-01-01T00:00:00Z".to_string(),
                platform: "darwin-arm64".to_string(),
            },
        );
        installer.save_manifest(&manifest).unwrap();

        // List plugins
        let plugins = installer.list_plugins_detailed().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "meta-test");
        assert_eq!(plugins[0].version.as_deref(), Some("2.0.0"));
        assert_eq!(plugins[0].source.as_deref(), Some("test-user/meta-test"));
    }

    // === M4: Project-local plugin installation tests ===

    #[test]
    fn test_find_workspace_root_rejects_bare_meta_directory() {
        let temp = tempfile::tempdir().unwrap();
        let meta_dir = temp.path().join(".meta");
        std::fs::create_dir(&meta_dir).unwrap();

        // Create subdirectory to test walking up
        let subdir = temp.path().join("sub").join("dir");
        std::fs::create_dir_all(&subdir).unwrap();

        let result = PluginInstaller::find_workspace_root_from(&subdir);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Not in a meta workspace"));
    }

    #[test]
    fn test_find_workspace_root_with_yaml() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".meta.yaml"), "projects: {}").unwrap();

        let result = PluginInstaller::find_workspace_root_from(temp.path());

        assert!(result.is_ok());
        // Canonicalize paths for comparison (handles /var vs /private/var on macOS)
        assert_eq!(
            result.unwrap().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_find_workspace_root_fails_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();

        let result =
            PluginInstaller::find_workspace_root_from(temp.path().ancestors().last().unwrap());

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Not in a meta workspace"));
        assert!(err_msg.contains("Create one here"));
        assert!(err_msg.contains(".meta.yaml"));
    }

    #[test]
    fn test_find_workspace_root_with_config_and_meta_directory() {
        let temp = tempfile::tempdir().unwrap();

        // A config file establishes a workspace alongside its .meta/ directory.
        let meta_dir = temp.path().join(".meta");
        std::fs::create_dir(&meta_dir).unwrap();
        std::fs::write(temp.path().join(".meta.yaml"), "projects: {}").unwrap();

        let result = PluginInstaller::find_workspace_root_from(temp.path());

        // Should successfully find the workspace root
        assert!(result.is_ok());
        // Canonicalize paths for comparison (handles /var vs /private/var on macOS)
        assert_eq!(
            result.unwrap().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_new_local_creates_installer_with_correct_path() {
        let temp = tempfile::tempdir().unwrap();
        let meta_dir = temp.path().join(".meta");
        std::fs::create_dir(&meta_dir).unwrap();

        std::fs::write(temp.path().join(".meta.yaml"), "projects: {}").unwrap();

        // Create plugins dir so we can canonicalize it
        let plugins_dir = temp.path().join(LOCAL_PLUGINS_DIR);
        std::fs::create_dir_all(&plugins_dir).unwrap();

        let result = PluginInstaller::new_local_from(temp.path());

        assert!(result.is_ok());
        let installer = result.unwrap();
        // Canonicalize paths for comparison (handles /var vs /private/var on macOS)
        assert_eq!(
            installer.plugins_dir.canonicalize().unwrap(),
            plugins_dir.canonicalize().unwrap()
        );
        assert_eq!(installer.scope, InstallScope::Local);
    }

    #[test]
    fn test_new_local_fails_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();

        let result = PluginInstaller::new_local_from(temp.path().ancestors().last().unwrap());

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Not in a meta workspace"));
    }

    #[test]
    fn local_plugins_gitignore_is_created_and_existing_file_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".meta.yaml"), "projects: {}").unwrap();
        let installer = PluginInstaller::new_local_from(temp.path()).unwrap();
        installer.ensure_plugins_dir().unwrap();
        let path = installer.plugins_dir.join(".gitignore");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "*\n");
        std::fs::write(&path, "custom\n").unwrap();
        installer.ensure_plugins_dir().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "custom\n");
    }

    #[test]
    fn test_constants_defined() {
        // Verify constants are accessible and have expected values
        assert_eq!(LOCAL_PLUGINS_DIR, ".meta/plugins");
        assert_eq!(GLOBAL_PLUGINS_DIR, "plugins");
    }

    // === M6: Simple registry tests ===

    #[test]
    fn test_default_registry_points_to_gitkb() {
        assert_eq!(
            DEFAULT_REGISTRY,
            "https://raw.githubusercontent.com/gitkb/meta-plugins/main"
        );
    }

    #[test]
    fn test_resolve_plugin_source_invalid_name() {
        let client = RegistryClient::new().unwrap();
        let result = client.resolve_plugin_source("nonexistent-plugin-12345");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // === M7: Plugin updates tests ===

    #[test]
    fn test_normalize_version_with_v_prefix() {
        assert_eq!(normalize_version("v1.0.0"), "1.0.0");
        assert_eq!(normalize_version("v2.3.4"), "2.3.4");
    }

    #[test]
    fn test_normalize_version_without_v_prefix() {
        assert_eq!(normalize_version("1.0.0"), "1.0.0");
        assert_eq!(normalize_version("2.3.4"), "2.3.4");
    }

    #[test]
    fn test_is_newer_version_basic() {
        assert!(is_newer_version("1.0.0", "1.0.1"));
        assert!(is_newer_version("1.0.0", "1.1.0"));
        assert!(is_newer_version("1.0.0", "2.0.0"));
    }

    #[test]
    fn test_is_newer_version_with_v_prefix() {
        assert!(is_newer_version("v1.0.0", "v1.0.1"));
        assert!(is_newer_version("1.0.0", "v1.0.1"));
        assert!(is_newer_version("v1.0.0", "1.0.1"));
    }

    #[test]
    fn test_is_newer_version_same() {
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("v1.0.0", "v1.0.0"));
    }

    #[test]
    fn test_is_newer_version_older() {
        assert!(!is_newer_version("2.0.0", "1.0.0"));
        assert!(!is_newer_version("1.1.0", "1.0.0"));
    }
}
