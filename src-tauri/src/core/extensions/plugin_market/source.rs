//! Where an index comes from: an `https://` address, or an index file or
//! market directory on a local or network drive (an offline lab copies the
//! directory). Nothing else is accepted, `http://` included: a package's
//! integrity rests on the SHA-256 the index lists, so the index must not be
//! open to tampering on the way. Package files resolve against the index:
//! a relative path stays next to it, an absolute URL must be `https://`.

use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest as _, Sha256};

use super::{index::INDEX_FILE, MarketError};

/// Longest wait for an index, and for one package download.
const INDEX_TIMEOUT: Duration = Duration::from_secs(30);
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A configured index location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceLocation {
    /// The index file's URL.
    Https(reqwest::Url),
    /// The index file's path.
    Local(PathBuf),
}

/// Where one package file is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageLocation {
    Https(reqwest::Url),
    Local(PathBuf),
}

impl std::fmt::Display for PackageLocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Https(url) => write!(formatter, "{url}"),
            Self::Local(path) => write!(formatter, "{}", path.display()),
        }
    }
}

impl SourceLocation {
    /// Parses a source as the user typed it. A directory, or an address
    /// ending in `/`, means its `index.json`.
    pub fn parse(text: &str) -> Result<Self, MarketError> {
        let text = text.trim();
        let invalid = |message: &str| MarketError::InvalidSource(format!("{text}: {message}"));
        if text.is_empty() {
            return Err(invalid("empty"));
        }
        if let Some((scheme, _)) = text.split_once("://") {
            if !scheme.eq_ignore_ascii_case("https") {
                return Err(invalid(
                    "only https:// addresses and local directories are accepted",
                ));
            }
            let mut url = reqwest::Url::parse(text).map_err(|error| invalid(&error.to_string()))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err(invalid("addresses must not carry credentials"));
            }
            if url.fragment().is_some() || url.query().is_some() {
                return Err(invalid("addresses must not carry a query or fragment"));
            }
            if url.path().ends_with('/') {
                url = url
                    .join(INDEX_FILE)
                    .map_err(|error| invalid(&error.to_string()))?;
            }
            return Ok(Self::Https(url));
        }
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            return Err(invalid("a local source is an absolute path"));
        }
        let is_index_file = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        Ok(Self::Local(if path.is_dir() || !is_index_file {
            path.join(INDEX_FILE)
        } else {
            path
        }))
    }

    /// The source's stable spelling: what is stored and shown.
    pub fn key(&self) -> String {
        match self {
            Self::Https(url) => url.to_string(),
            Self::Local(path) => path.to_string_lossy().into_owned(),
        }
    }

    /// Where `url` (from the index) points.
    pub fn resolve(&self, url: &str) -> Result<PackageLocation, MarketError> {
        let invalid = |message: String| MarketError::InvalidIndex {
            location: self.key(),
            message,
        };
        if url.contains("://") {
            let parsed = reqwest::Url::parse(url).map_err(|error| invalid(error.to_string()))?;
            if parsed.scheme() != "https" {
                return Err(invalid(format!("{url}: only https:// URLs are allowed")));
            }
            return Ok(PackageLocation::Https(parsed));
        }
        match self {
            Self::Https(index) => {
                let joined = index
                    .join(url)
                    .map_err(|error| invalid(error.to_string()))?;
                if joined.scheme() != "https" {
                    return Err(invalid(format!("{url}: only https:// URLs are allowed")));
                }
                Ok(PackageLocation::Https(joined))
            }
            Self::Local(index) => {
                let directory = index.parent().unwrap_or(Path::new(""));
                Ok(PackageLocation::Local(
                    url.split('/')
                        .fold(directory.to_owned(), |path, part| path.join(part)),
                ))
            }
        }
    }

    /// The index document, at most `limit` bytes.
    pub async fn read_index(
        &self,
        client: &reqwest::Client,
        limit: u64,
    ) -> Result<Vec<u8>, MarketError> {
        let fetch = |message: String| MarketError::Fetch {
            location: self.key(),
            message,
        };
        match self {
            Self::Https(url) => {
                let response = client
                    .get(url.clone())
                    .timeout(INDEX_TIMEOUT)
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|error| fetch(error.to_string()))?;
                read_body(response, limit)
                    .await
                    .map_err(fetch)?
                    .ok_or_else(|| fetch(format!("the index is larger than {limit} bytes")))
            }
            Self::Local(path) => {
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    let file = std::fs::File::open(&path)?;
                    let mut bytes = Vec::new();
                    file.take(limit + 1).read_to_end(&mut bytes)?;
                    Ok::<_, std::io::Error>(bytes)
                })
                .await
                .map_err(|error| fetch(error.to_string()))?
                .map_err(|error| fetch(error.to_string()))
                .and_then(|bytes| {
                    if bytes.len() as u64 > limit {
                        Err(fetch(format!("the index is larger than {limit} bytes")))
                    } else {
                        Ok(bytes)
                    }
                })
            }
        }
    }
}

/// A response body of at most `limit` bytes; `None` when it is longer.
async fn read_body(mut response: reqwest::Response, limit: u64) -> Result<Option<Vec<u8>>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

/// Copies or downloads the package at `location` into `destination`,
/// requiring exactly `size` bytes with SHA-256 `sha256`. The bytes go to a
/// temporary file next to `destination` that is renamed only after they
/// matched, so `destination` never holds unverified bytes.
pub async fn fetch_package(
    client: &reqwest::Client,
    location: &PackageLocation,
    size: u64,
    sha256: &str,
    destination: &Path,
) -> Result<(), MarketError> {
    let partial = destination.with_extension(format!("part-{}", uuid::Uuid::new_v4().simple()));
    let fetched = transfer(client, location, size, &partial).await;
    let verified = fetched.and_then(|actual| {
        if actual.size == size && actual.sha256 == sha256 {
            Ok(())
        } else {
            Err(MarketError::Integrity {
                url: location.to_string(),
                message: format!(
                    "{} bytes with SHA-256 {}, the index lists {size} bytes with {sha256}",
                    actual.size, actual.sha256
                ),
            })
        }
    });
    let finished =
        verified.and_then(|()| std::fs::rename(&partial, destination).map_err(MarketError::from));
    if finished.is_err() {
        if let Err(error) = std::fs::remove_file(&partial) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %partial.display(), "partial package not removed: {error}");
            }
        }
    }
    finished
}

/// The size and SHA-256 of what was written.
struct Written {
    size: u64,
    sha256: String,
}

/// Writes at most `limit` + 1 bytes from `location` to `target`.
async fn transfer(
    client: &reqwest::Client,
    location: &PackageLocation,
    limit: u64,
    target: &Path,
) -> Result<Written, MarketError> {
    let failed = |message: String| MarketError::Download {
        url: location.to_string(),
        message,
    };
    match location {
        PackageLocation::Https(url) => {
            let mut response = client
                .get(url.clone())
                .timeout(PACKAGE_TIMEOUT)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| failed(error.to_string()))?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            let mut hasher = Sha256::new();
            let mut size = 0_u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| failed(error.to_string()))?
            {
                size += chunk.len() as u64;
                if size > limit {
                    return Err(failed(format!(
                        "the file is larger than the {limit} bytes listed"
                    )));
                }
                hasher.update(&chunk);
                file.write_all(&chunk)?;
            }
            file.flush()?;
            Ok(Written {
                size,
                sha256: hex::encode(hasher.finalize()),
            })
        }
        PackageLocation::Local(path) => {
            let path = path.clone();
            let target = target.to_owned();
            let location = location.to_string();
            tokio::task::spawn_blocking(move || -> Result<Written, MarketError> {
                let mut source = std::fs::File::open(&path)?.take(limit + 1);
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                let mut hasher = Sha256::new();
                let mut buffer = vec![0_u8; 256 * 1024];
                let mut size = 0_u64;
                loop {
                    let length = source.read(&mut buffer)?;
                    if length == 0 {
                        break;
                    }
                    size += length as u64;
                    if size > limit {
                        return Err(MarketError::Download {
                            url: location,
                            message: format!("the file is larger than the {limit} bytes listed"),
                        });
                    }
                    hasher.update(&buffer[..length]);
                    file.write_all(&buffer[..length])?;
                }
                file.flush()?;
                Ok(Written {
                    size,
                    sha256: hex::encode(hasher.finalize()),
                })
            })
            .await
            .map_err(|error| failed(error.to_string()))?
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_https_or_absolute_local_paths() {
        assert_eq!(
            SourceLocation::parse("https://plugins.example.com/market/")
                .expect("https")
                .key(),
            "https://plugins.example.com/market/index.json"
        );
        assert_eq!(
            SourceLocation::parse("  https://plugins.example.com/index.json ")
                .expect("https")
                .key(),
            "https://plugins.example.com/index.json"
        );
        let directory = tempfile::tempdir().expect("market");
        assert_eq!(
            SourceLocation::parse(&directory.path().to_string_lossy()).expect("local"),
            SourceLocation::Local(directory.path().join(INDEX_FILE))
        );
        let file = directory.path().join("lab.json");
        assert_eq!(
            SourceLocation::parse(&file.to_string_lossy()).expect("index file"),
            SourceLocation::Local(file)
        );
        for bad in [
            "",
            "http://plugins.example.com/index.json",
            "file:///C:/market/index.json",
            "https://user:secret@plugins.example.com/index.json",
            "https://plugins.example.com/index.json?token=1",
            "market/index.json",
        ] {
            assert!(
                SourceLocation::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn package_files_resolve_next_to_the_index() {
        let https =
            SourceLocation::parse("https://plugins.example.com/market/index.json").expect("https");
        assert_eq!(
            https.resolve("files/demo.lcpkg").expect("relative"),
            PackageLocation::Https(
                reqwest::Url::parse("https://plugins.example.com/market/files/demo.lcpkg")
                    .expect("url")
            )
        );
        assert!(https
            .resolve("http://elsewhere.example/demo.lcpkg")
            .is_err());
        let directory = tempfile::tempdir().expect("market");
        let local = SourceLocation::Local(directory.path().join(INDEX_FILE));
        assert_eq!(
            local.resolve("files/demo.lcpkg").expect("relative"),
            PackageLocation::Local(directory.path().join("files").join("demo.lcpkg"))
        );
    }

    #[tokio::test]
    async fn packages_are_kept_only_when_size_and_digest_match() {
        let directory = tempfile::tempdir().expect("market");
        let source = directory.path().join("demo.lcpkg");
        std::fs::write(&source, b"package bytes").expect("package");
        let sha256 = hex::encode(Sha256::digest(b"package bytes"));
        let client = reqwest::Client::new();
        let location = PackageLocation::Local(source);
        let destination = directory.path().join("copy.lcpkg");
        fetch_package(&client, &location, 13, &sha256, &destination)
            .await
            .expect("verified copy");
        assert_eq!(std::fs::read(&destination).expect("copy"), b"package bytes");

        let tampered = directory.path().join("tampered.lcpkg");
        let error = fetch_package(&client, &location, 13, &"0".repeat(64), &tampered)
            .await
            .expect_err("digest mismatch");
        assert!(matches!(error, MarketError::Integrity { .. }), "{error}");
        let error = fetch_package(&client, &location, 4, &sha256, &tampered)
            .await
            .expect_err("longer than listed");
        assert!(matches!(error, MarketError::Download { .. }), "{error}");
        assert!(!tampered.exists());
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("market").count(),
            2,
            "no partial files remain"
        );
    }
}
