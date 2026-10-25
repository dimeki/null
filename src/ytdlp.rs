use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: u32,
    /// Cover art URL; empty means "use the video thumbnail"
    pub thumb: String,
}

#[derive(Clone)]
pub struct YtDlp {
    exe: PathBuf,
    /// Signed-in cookies, when the user has imported them
    cookies: Option<PathBuf>,
}

impl YtDlp {
    pub fn new(exe: &Path) -> Self {
        YtDlp { exe: exe.to_path_buf(), cookies: None }
    }

    pub fn set_cookies(&mut self, cookies: Option<PathBuf>) {
        self.cookies = cookies;
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new(&self.exe);
        if let Some(deno) = self.exe.parent().map(|d| d.join("deno.exe")).filter(|p| p.exists()) {
            cmd.arg("--js-runtimes").arg(format!("deno:{}", deno.display()));
        }
        // without this yt-dlp prints in the legacy Windows code page, which breaks accented titles
        cmd.args(["--encoding", "utf-8"]);
        // yt-dlp rewrites its cookie file on exit, and several runs can overlap,
        // so each run gets its own throwaway copy
        let cookie_copy = self.cookies.as_ref().and_then(|src| {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let copy = src.with_file_name(format!("run-{}-{nanos}.tmp", std::process::id()));
            std::fs::copy(src, &copy).ok().map(|_| copy)
        });
        if let Some(copy) = &cookie_copy {
            cmd.arg("--cookies").arg(copy);
        }
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = cmd.output();
        if let Some(copy) = cookie_copy {
            let _ = std::fs::remove_file(copy);
        }
        let out = out.map_err(|e| format!("run yt-dlp: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn version(&self) -> Result<String, String> {
        self.run(&["--version"]).map(|v| v.trim().to_string())
    }

    /// Runs yt-dlp's own updater and returns the last line it printed
    pub fn update(&self) -> Result<String, String> {
        let out = self.run(&["-U"])?;
        Ok(out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Updated").trim().to_string())
    }

    /// Lets yt-dlp read the browser's cookies and write them to `dest` (Netscape format)
    pub fn dump_browser_cookies(&self, browser: &str, dest: &Path) -> Result<(), String> {
        let mut plain = self.clone();
        plain.cookies = None;
        plain
            .run(&[
                "--cookies-from-browser",
                browser,
                "--cookies",
                &dest.to_string_lossy(),
                "--skip-download",
                "--no-playlist",
                "--no-warnings",
                "-q",
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            ])
            .map(|_| ())
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>, String> {
        let q = format!("ytsearch{limit}:{query}");
        let out = self.run(&[
            &q,
            "--flat-playlist",
            "--print",
            "%(id)s\t%(title)s\t%(channel)s\t%(duration)s",
        ])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\t');
                Some(Track {
                    id: p.next()?.to_string(),
                    title: p.next()?.to_string(),
                    channel: p.next()?.to_string(),
                    duration: p.next()?.parse().unwrap_or(0),
                    thumb: String::new(),
                })
            })
            .collect())
    }

    /// Downloads a track's audio into `cache_dir` (skipped if already cached)
    /// and returns the local file. yt-dlp does the fetching because YouTube's
    /// stream URLs only work inside the session that produced them.
    pub fn fetch_audio(&self, id: &str, cache_dir: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(cache_dir).map_err(|e| format!("create cache dir: {e}"))?;
        if let Some(hit) = self.cached(id, cache_dir) {
            return Ok(hit);
        }
        let template = cache_dir.join(format!("{id}.%(ext)s"));
        let url = format!("https://www.youtube.com/watch?v={id}");
        self.run(&[
            "-f",
            "bestaudio",
            "--no-playlist",
            "--no-progress",
            "--no-part",
            "-o",
            &template.to_string_lossy(),
            &url,
        ])?;
        self.cached(id, cache_dir)
            .ok_or_else(|| "download finished but file is missing".to_string())
    }

    pub fn cached(&self, id: &str, cache_dir: &Path) -> Option<PathBuf> {
        find_cached(cache_dir, id)
    }
}

/// Deletes the oldest cached files until the folder fits in `max_bytes`.
pub fn prune_cache(dir: &Path, max_bytes: u64) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = read
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

fn find_cached(dir: &Path, id: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().is_some_and(|s| s == id) && p.extension().is_some_and(|e| e != "part" && e != "ytdl")
    })
}
