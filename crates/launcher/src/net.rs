//! Downloads over HTTPS with Windows' own WinHTTP (no TLS stack of our
//! own), and the project's releases from the GitHub API.

use anyhow::{Context, Result, bail};

/// The repository releases come from.
pub const REPO: &str = "FrosttysBots/Disney-Extreme-Skaters-Recomp";

/// The two programs each release carries, and the launcher itself.
pub const GAME_ASSET: &str = "Disney-Extreme-Skate-Adventure.exe";
pub const VIEWER_ASSET: &str = "DESA-Map-Viewer.exe";
pub const LAUNCHER_ASSET: &str = "DESA-Launcher.exe";

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub name: String,
    /// The notes, as markdown.
    pub body: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Whether it has both programs to install.
    pub fn playable(&self) -> bool {
        self.asset(GAME_ASSET).is_some() && self.asset(VIEWER_ASSET).is_some()
    }
}

/// The repository's releases, newest first.
pub fn releases() -> Result<Vec<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=50");
    let bytes = get(&url, &mut |_, _| true)?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).context("GitHub's answer isn't JSON")?;
    let list = json.as_array().context("GitHub's answer isn't a list")?;
    let text = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let mut out: Vec<Release> = list
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .map(|r| Release {
            tag: text(r, "tag_name"),
            name: text(r, "name"),
            body: text(r, "body").replace("\r\n", "\n"),
            date: text(r, "published_at").chars().take(10).collect(),
            prerelease: r["prerelease"].as_bool().unwrap_or(false),
            assets: r["assets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|a| Asset {
                            name: text(a, "name"),
                            url: text(a, "browser_download_url"),
                            size: a["size"].as_u64().unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    out.sort_by(|a, b| compare_tags(&b.tag, &a.tag));
    Ok(out)
}

/// Orders tags like `v0.1.13` by their numbers.
pub fn compare_tags(a: &str, b: &str) -> std::cmp::Ordering {
    let nums = |t: &str| -> Vec<u64> {
        t.trim_start_matches(['v', 'V'])
            .split(['.', '-'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    nums(a).cmp(&nums(b))
}

/// GETs `url` (https only), calling `progress(done, total)` as it comes
/// in; `progress` returning false cancels.
#[cfg(windows)]
pub fn get(url: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Networking::WinHttp::*;

    let rest = url
        .strip_prefix("https://")
        .context("only https downloads")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = format!("/{path}");
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain([0]).collect() };

    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let last = || std::io::Error::last_os_error();

    unsafe {
        let agent = wide("DESA-Launcher");
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            bail!("couldn't start WinHTTP: {}", last());
        }
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
        let host_w = wide(host);
        let connection = Handle(WinHttpConnect(session.0, host_w.as_ptr(), 443, 0));
        if connection.0.is_null() {
            bail!("couldn't reach {host}: {}", last());
        }
        let verb = wide("GET");
        let path_w = wide(&path);
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path_w.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            bail!("couldn't make the request: {}", last());
        }
        let headers = wide("Accept: application/vnd.github+json, application/octet-stream\r\n");
        WinHttpAddRequestHeaders(
            request.0,
            headers.as_ptr(),
            u32::MAX,
            WINHTTP_ADDREQ_FLAG_ADD,
        );
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            bail!("no answer from {host} (are you online?): {}", last());
        }
        let number = |level: u32| -> Option<u64> {
            let mut value: u32 = 0;
            let mut len = 4u32;
            let ok = WinHttpQueryHeaders(
                request.0,
                level | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut value as *mut u32).cast(),
                &mut len,
                null_mut(),
            );
            (ok != 0).then_some(u64::from(value))
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        if status != 200 {
            bail!("{host} answered {status}");
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH);
        let mut out = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            ) == 0
            {
                bail!("the download broke off: {}", last());
            }
            if read == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..read as usize]);
            if !progress(out.len() as u64, total) {
                bail!("cancelled");
            }
        }
        if let Some(total) = total.filter(|&t| t != out.len() as u64) {
            bail!("the download stopped at {} of {total} bytes", out.len());
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
pub fn get(_url: &str, _progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>> {
    bail!("downloads are only written for Windows so far")
}

#[cfg(test)]
mod tests {
    use super::compare_tags;
    use std::cmp::Ordering;

    #[test]
    fn tags_order_by_number() {
        assert_eq!(compare_tags("v0.1.13", "v0.1.9"), Ordering::Greater);
        assert_eq!(compare_tags("v0.1.9", "v0.1.10"), Ordering::Less);
        assert_eq!(compare_tags("v0.2.0", "v0.1.99"), Ordering::Greater);
    }
}
