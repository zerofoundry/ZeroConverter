// ==============================================================================
// OWNERSHIP:
//   Rename-to-convert: rename `photo.png` to `photo.webp` in File Explorer,
//   anywhere on a local drive, and the file really becomes a WebP. In the
//   user's own folders (Desktop, Downloads, Documents, Pictures, Videos,
//   Music, plus any they add) renames from any app count. Wires the watcher, the
//   decision rule and the swap together, and talks to the shell
//   (notifications, the `+` options window).
// ==============================================================================
pub mod apply;
mod desktop;
pub mod rule;
mod shell_notice;
pub mod watch;

use crate::core::app_error::AppError;

use crate::core::conversion::registry::ConverterRegistry;
use crate::core::conversion::types::{find_descriptor, large_input, AssetFamily};
use crate::i18n::{join_sentences, tr, tr_n};
use crate::receipt::Tone;
use apply::{Job, QuickOptions};
use rule::Decision;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};

static KNOWN_EXTENSIONS: OnceLock<HashSet<String>> = OnceLock::new();
static DEBUG_START: OnceLock<std::time::Instant> = OnceLock::new();

/// A folder conversion refuses beyond this many files: renaming a whole
/// library by accident should not send thousands of originals to the bin.
const MAX_FOLDER_FILES: usize = 2_000;

/// Number of sniff attempts on a rename before treating read failures as unreadable.
const SNIFF_ATTEMPTS: usize = 7;

/// Delay between transient sniff retries (~300 ms max across 6 waits).
const SNIFF_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// How long a renamed folder or file waits for Explorer to announce that
/// rename before moving it away. Explorer's notice comes 0.2 to 0.6 s after
/// the rename; 1.5 s after it Explorer is done even when no notice was seen.
pub(crate) const EXPLORER_RENAME_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// The pause after Explorer's notice. Renamed back with no pause, about one
/// rename in ten on the Desktop still got the error box.
pub(crate) const EXPLORER_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

pub struct RenameConvert {
    registry: Arc<ConverterRegistry>,
    pub enabled: AtomicBool,
    pub keep_original: AtomicBool,
    /// The app is closing (Quit, or an update): no file is swapped any more.
    quitting: AtomicBool,
    in_flight: Mutex<HashSet<PathBuf>>,
    waiting: Mutex<HashMap<String, Job>>,
    watcher: OnceLock<watch::Watcher>,
    /// How many single renames and menu copies convert at once.
    slots: Slots,
}

/// A counting gate: a burst of renames (a script, a sync tool) converts a
/// few at a time instead of all at once. Folder conversions have their own
/// worker pool of the same size.
struct Slots {
    used: Mutex<usize>,
    freed: std::sync::Condvar,
    max: usize,
}

struct Slot<'a>(&'a Slots);

impl Slots {
    fn new(max: usize) -> Self {
        Self {
            used: Mutex::new(0),
            freed: std::sync::Condvar::new(),
            max: max.max(1),
        }
    }

    /// Waits for a free slot. `None` once `give_up` says so: the job then
    /// runs straight into its own stop check and changes nothing.
    fn take(&self, give_up: impl Fn() -> bool) -> Option<Slot<'_>> {
        let mut used = self.used.lock().unwrap();
        loop {
            if *used < self.max {
                *used += 1;
                return Some(Slot(self));
            }
            if give_up() {
                return None;
            }
            used = self
                .freed
                .wait_timeout(used, std::time::Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.used.lock().unwrap() -= 1;
        self.0.freed.notify_one();
    }
}

/// One watched folder, for the settings list.
#[derive(serde::Serialize)]
pub struct WatchedFolder {
    pub path: String,
    /// One of the user's own folders: always watched, cannot be removed.
    pub builtin: bool,
}

#[derive(serde::Serialize)]
pub struct QuickJobInfo {
    pub name: String,
    pub source: String,
    pub target: String,
    pub family: String,
    /// Over 100 MB: the window asks before converting.
    pub large_bytes: Option<u64>,
    /// What the engine lets the user set for this target; the window shows
    /// the ones marked `quick`.
    pub options: Vec<&'static crate::core::conversion::options::OptionSpec>,
}

impl RenameConvert {
    pub fn new(registry: Arc<ConverterRegistry>) -> Arc<Self> {
        let _ = DEBUG_START.get_or_init(std::time::Instant::now);
        let mut known = HashSet::new();
        for family in [AssetFamily::Image, AssetFamily::Audio, AssetFamily::Video] {
            if let Some(h) = registry.get_handler(&family) {
                for d in h.format_descriptors() {
                    known.insert(d.canonical_extension.to_string());
                    known.extend(d.aliases.iter().map(|a| a.to_string()));
                }
            }
        }
        let _ = KNOWN_EXTENSIONS.set(known);
        Arc::new(Self {
            registry,
            enabled: AtomicBool::new(true),
            keep_original: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            in_flight: Mutex::new(HashSet::new()),
            waiting: Mutex::new(HashMap::new()),
            watcher: OnceLock::new(),
            slots: Slots::new(std::thread::available_parallelism().map_or(4, |n| n.get())),
        })
    }

    /// Starts the folder watchers and the dispatcher thread.
    pub fn start(self: &Arc<Self>, app: AppHandle) {
        let (tx, rx) = std::sync::mpsc::channel::<watch::Rename>();
        let watcher = watch::Watcher::new(tx, quick_filter);
        watcher.set_folders(&self.all_folders(&crate::settings::load(&app).rename_folders));
        let _ = self.watcher.set(watcher);
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("zc-rename-convert".into())
            .spawn(move || {
                for event in rx {
                    if this.enabled.load(Ordering::Relaxed) {
                        this.handle(&app, event);
                    }
                }
            })
            .ok();
    }

    fn handle(self: &Arc<Self>, app: &AppHandle, event: watch::Rename) {
        if event.new.is_dir() {
            self.handle_folder(app, &event);
            return;
        }
        if !rule::may_need_sniff(&event.old, &event.new, |id| self.known(id)) {
            return;
        }
        // Explorer or antivirus may still hold the file for a moment after the rename.
        let content = retry_read(
            SNIFF_ATTEMPTS,
            SNIFF_RETRY_DELAY,
            || crate::core::sniff::try_sniff_file(&event.new),
            std::thread::sleep,
        )
        .map_or(rule::Content::Unreadable, |kind| {
            kind.map_or(rule::Content::Unknown, rule::Content::Known)
        });
        let admitted_id = apply::capture_identity(&event.new);
        debug(&format!("event {:?} content={content:?}", event));
        let decision = rule::decide(
            &event.old,
            &event.new,
            content,
            |id| self.known(id),
            |s, t| self.can_route(s, t),
        );
        debug(&format!("decision {decision:?}"));
        match decision {
            Decision::Ignore => {}
            Decision::Restore => {
                let _ = restore(&event.new, &event.old, event.at, admitted_id, false);
            }
            Decision::Unsupported { .. } | Decision::Unreadable | Decision::Mismatch { .. } => {
                let allow_unreadable = matches!(decision, Decision::Unreadable);
                let restored = restore(
                    &event.new,
                    &event.old,
                    event.at,
                    admitted_id,
                    allow_unreadable,
                );
                if let Some((title, body)) =
                    decision_notice(&decision, &event.old, &event.new, restored)
                {
                    notify(app, Tone::Warning, &title, &body);
                }
            }
            Decision::Convert {
                source,
                target,
                modifiers,
                final_path,
            } => {
                if modifiers.max_size.is_some()
                    && self.registry.resolve_family(&source) != AssetFamily::Image
                {
                    let restored = restore(&event.new, &event.old, event.at, admitted_id, false);
                    let body = if restored {
                        tr("rename.sizePicturesBody", &[])
                    } else {
                        tr("rename.stillNamed", &[("name", &base_name(&event.new))])
                    };
                    notify(
                        app,
                        Tone::Warning,
                        &tr("rename.sizePicturesTitle", &[]),
                        &body,
                    );
                    return;
                }
                let Some(admitted_id) = admitted_id else {
                    let restored = restore(&event.new, &event.old, event.at, None, true);
                    if let Some((title, body)) =
                        decision_notice(&Decision::Unreadable, &event.old, &event.new, restored)
                    {
                        notify(app, Tone::Warning, &title, &body);
                    }
                    return;
                };
                let job = self.build_rename_job(
                    &event.old,
                    &event.new,
                    &source,
                    target,
                    modifiers,
                    &final_path,
                    Some(event.at),
                    Some(admitted_id),
                );
                if !self.in_flight.lock().unwrap().insert(job.current.clone()) {
                    return;
                }
                // Over 100 MB the options window doubles as the question.
                if modifiers.ask_options || large_input(&job.current).is_some() {
                    let id = uuid::Uuid::new_v4().simple().to_string();
                    self.waiting.lock().unwrap().insert(id.clone(), job);
                    open_quick_window(app, &id);
                } else {
                    self.spawn(app.clone(), job, QuickOptions::default());
                }
            }
        }
    }

    fn should_keep_original(&self, has_keep_mark: bool) -> bool {
        keep_original_for(self.keep_original.load(Ordering::Relaxed), has_keep_mark)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_rename_job(
        &self,
        old: &Path,
        new: &Path,
        source: &str,
        target: String,
        modifiers: rule::Modifiers,
        final_path: &Path,
        renamed_at: Option<std::time::Instant>,
        admitted_id: Option<apply::FileIdentity>,
    ) -> Job {
        let keep_original = self.should_keep_original(modifiers.keep_original);
        Job {
            current: new.to_path_buf(),
            original: old.to_path_buf(),
            final_path: apply::final_path_for(final_path, old, keep_original),
            source: self.canonical(source),
            target,
            keep_original,
            max_size: modifiers.max_size,
            from_menu: false,
            renamed_at,
            admitted_id,
        }
    }

    /// The spelling a converter was built for (`jpeg` → `jpg`).
    fn canonical(&self, source: &str) -> String {
        let family = self.registry.resolve_family(source);
        self.registry
            .get_handler(&family)
            .and_then(|h| {
                find_descriptor(h.format_descriptors(), source)
                    .map(|d| d.canonical_extension.to_string())
            })
            .unwrap_or_else(|| source.to_string())
    }

    /// `Holiday` → `Holiday.webp`: the folder gets its name back and every
    /// file in it (subfolders too) that can become a WebP does.
    fn handle_folder(self: &Arc<Self>, app: &AppHandle, event: &watch::Rename) {
        let Some(plan) = rule::decide_folder(&event.old, &event.new, |id| self.writable(id)) else {
            return;
        };
        debug(&format!("folder {:?} -> {plan:?}", event.new));
        let folder = rename_back(&event.new, &event.old);
        let name = event
            .old
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let upper = plan.target.to_uppercase();

        let Some(FolderPlan {
            jobs,
            taken,
            large,
            keep,
        }) = self.plan_folder(&folder, &plan)
        else {
            notify(
                app,
                Tone::Warning,
                &tr("rename.folderTooBigTitle", &[("name", &name)]),
                &tr(
                    "rename.folderTooBigBody",
                    &[("max", &MAX_FOLDER_FILES.to_string())],
                ),
            );
            return;
        };
        debug(&format!("folder planned: {} jobs", jobs.len()));
        if jobs.is_empty() {
            notify(
                app,
                Tone::Warning,
                &tr("rename.folderNothingTitle", &[("name", &name)]),
                &if large > 0 {
                    tr("rename.folderNothingLarge", &[])
                } else {
                    tr("rename.folderNothingNone", &[("format", &upper)])
                },
            );
            return;
        }

        let this = Arc::clone(self);
        let app = app.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let counts = this.run_folder(&jobs, keep);
            if counts.no_ffmpeg > 0 {
                crate::tray::open_settings(&app, "video");
            }
            let (tone, body) = folder_summary(&counts, large, taken, keep, started.elapsed());
            notify(&app, tone, &format!("{name} → {upper}"), &body);
        });
    }

    /// The files of `folder` that become `plan.target`. `None` when the
    /// folder holds too many files.
    fn plan_folder(&self, folder: &Path, plan: &rule::FolderConvert) -> Option<FolderPlan> {
        let files = folder_files(folder);
        if files.len() > MAX_FOLDER_FILES {
            return None;
        }
        let keep = self.should_keep_original(plan.modifiers.keep_original);
        let mut jobs = Vec::new();
        let mut taken = 0usize;
        let mut large = 0usize;
        // Final names already given to a job: `a.png` and `a.jpg` both want
        // `a.webp`, and only the first may have it (names ignore case).
        let mut claimed = HashSet::new();
        for file in files {
            let Some(content) = crate::core::sniff::sniff_file(&file) else {
                continue;
            };
            if crate::core::sniff::same_format(content, &plan.target)
                || !self.can_route(content, &plan.target)
                || (plan.modifiers.max_size.is_some()
                    && self.registry.resolve_family(content) != AssetFamily::Image)
            {
                continue;
            }
            // A folder convert never asks per file: large ones wait for
            // the user to convert them one by one.
            if large_input(&file).is_some() {
                large += 1;
                continue;
            }
            let final_path = file.with_extension(&plan.target);
            if final_path.exists() || !claimed.insert(final_path.to_string_lossy().to_lowercase()) {
                taken += 1;
                continue;
            }
            let Some(admitted_id) = apply::capture_identity(&file) else {
                continue;
            };
            jobs.push(Job {
                current: file.clone(),
                original: file,
                final_path,
                source: self.canonical(content),
                target: plan.target.clone(),
                keep_original: keep,
                max_size: plan.modifiers.max_size,
                from_menu: false,
                renamed_at: None,
                admitted_id: Some(admitted_id),
            });
        }
        Some(FolderPlan {
            jobs,
            taken,
            large,
            keep,
        })
    }

    /// Converts the jobs where they lie, a few at a time.
    fn run_folder(&self, jobs: &[Job], keep: bool) -> FolderCounts {
        // FFmpeg already uses every core; pictures and audio go in parallel.
        let workers = if jobs
            .iter()
            .any(|j| self.registry.resolve_family(&j.source) == AssetFamily::Video)
        {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(jobs.len())
        };
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (done, failed, not_binned, stopped, no_ffmpeg) = (
            std::sync::atomic::AtomicUsize::new(0),
            std::sync::atomic::AtomicUsize::new(0),
            std::sync::atomic::AtomicUsize::new(0),
            std::sync::atomic::AtomicUsize::new(0),
            std::sync::atomic::AtomicUsize::new(0),
        );
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    self.in_flight.lock().unwrap().insert(job.current.clone());
                    let result =
                        apply::run_unless(job, &self.registry, &QuickOptions::default(), &|| {
                            self.stopped()
                        });
                    self.in_flight.lock().unwrap().remove(&job.current);
                    let counter = match &result {
                        Ok(d) => {
                            if !keep && !d.original_recycled {
                                not_binned.fetch_add(1, Ordering::Relaxed);
                            }
                            &done
                        }
                        Err(AppError::Cancelled) => &stopped,
                        Err(e) if needs_ffmpeg(e) => &no_ffmpeg,
                        Err(e) => {
                            debug(&format!("folder job {:?} failed: {e}", job.current));
                            &failed
                        }
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        FolderCounts {
            done: done.into_inner(),
            failed: failed.into_inner(),
            not_binned: not_binned.into_inner(),
            stopped: stopped.into_inner(),
            no_ffmpeg: no_ffmpeg.into_inner(),
        }
    }

    fn spawn(self: &Arc<Self>, app: AppHandle, job: Job, options: QuickOptions) {
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            let from = job
                .original
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let menu = job.from_menu;
            let stop = || {
                if menu {
                    this.quitting.load(Ordering::Relaxed)
                } else {
                    this.stopped()
                }
            };
            // Waits its turn in a burst; a stop while waiting skips the wait.
            let _slot = this.slots.take(stop);
            let result = apply::run_outcome(&job, &this.registry, &options, &stop);
            if let Err(ref failure) = result {
                if failure.restored && !menu && !this.quitting.load(Ordering::Relaxed) {
                    shell_notice::file_restored(&job.current, &job.original);
                }
            }
            match result {
                Ok(done) => {
                    if !job.from_menu {
                        let kept = done.kept_original.as_deref();
                        shell_notice::file_converted(
                            &job.current,
                            &done.final_path,
                            kept.unwrap_or(&job.original),
                            kept.is_some(),
                        );
                    }
                    let to = done
                        .final_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    notify(
                        &app,
                        Tone::Success,
                        &format!("{from} → {to}"),
                        &success_card_body(job.keep_original, &done),
                    );
                }
                Err(failure) => match failure.error {
                    AppError::Cancelled => notify(
                        &app,
                        Tone::Warning,
                        &tr("rename.notConvertedTitle", &[]),
                        &tr(
                            "rename.notConvertedBody",
                            &[
                                ("from", &from),
                                (
                                    "reason",
                                    &tr(
                                        if this.quitting.load(Ordering::Relaxed) {
                                            "rename.reasonClosing"
                                        } else {
                                            "rename.reasonOff"
                                        },
                                        &[],
                                    ),
                                ),
                            ],
                        ),
                    ),
                    e if needs_ffmpeg(&e) => {
                        crate::tray::open_settings(&app, "video");
                        notify(
                            &app,
                            Tone::Warning,
                            &tr("rename.ffmpegTitle", &[]),
                            &join_sentences(&[
                                tr("rename.ffmpegBody", &[("from", &from)]),
                                if !menu && failure.restored {
                                    tr("rename.nameBack", &[])
                                } else {
                                    String::new()
                                },
                            ]),
                        )
                    }
                    e => {
                        // Say where the file really is, not where it should be.
                        let place = if failure.restored {
                            tr("rename.nameBack", &[])
                        } else if job.current.exists() {
                            tr("rename.stillNamed", &[("name", &base_name(&job.current))])
                        } else {
                            String::new()
                        };
                        notify(
                            &app,
                            Tone::Failure,
                            &tr("rename.failedTitle", &[]),
                            &join_sentences(&[
                                tr(
                                    "rename.failedBody",
                                    &[("from", &from), ("error", &e.to_string())],
                                ),
                                place,
                            ]),
                        )
                    }
                },
            }
            this.in_flight.lock().unwrap().remove(&job.current);
        });
    }

    /// Off, or the app is quitting: jobs that have not swapped stop.
    fn stopped(&self) -> bool {
        !self.enabled.load(Ordering::Relaxed) || self.quitting.load(Ordering::Relaxed)
    }

    /// Before the app closes: no file is swapped from now on, renames waiting
    /// for the options window get their names back, and a swap already under
    /// way may finish (up to `max`).
    pub fn shut_down(&self, max: std::time::Duration) {
        self.quitting.store(true, Ordering::Relaxed);
        let waiting: Vec<Job> = self
            .waiting
            .lock()
            .unwrap()
            .drain()
            .map(|(_, j)| j)
            .collect();
        for job in waiting {
            if job
                .admitted_id
                .is_some_and(|fid| apply::matches_identity(&job.current, fid))
            {
                if let Some(at) = job.renamed_at.filter(|_| !job.from_menu) {
                    apply::wait_for_file_notice(&job.current, at);
                }
                let _ = apply::restore_verified(&job);
            }
            self.in_flight.lock().unwrap().remove(&job.current);
        }
        let deadline = std::time::Instant::now() + max;
        while !self.in_flight.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// The app did not close after all (an update failed to install): files
    /// are swapped again.
    pub fn resume(&self) {
        self.quitting.store(false, Ordering::Relaxed);
    }

    pub fn quick_info(&self, id: &str) -> Option<QuickJobInfo> {
        let waiting = self.waiting.lock().unwrap();
        let job = waiting.get(id)?;
        let family = self.registry.resolve_family(&job.source);
        let options = self
            .registry
            .get_handler(&family)
            .map(|handler| handler.output_options(&job.target))
            .unwrap_or_default();
        Some(QuickJobInfo {
            name: job.original.file_name()?.to_string_lossy().into_owned(),
            source: job.source.clone(),
            target: job.target.clone(),
            family: format!("{family:?}").to_lowercase(),
            large_bytes: large_input(&job.current),
            options,
        })
    }

    pub fn quick_confirm(self: &Arc<Self>, app: &AppHandle, id: &str, mut options: QuickOptions) {
        if let Some(job) = self.waiting.lock().unwrap().remove(id) {
            // Pressing Convert in the window is the consent for a large file.
            options.0.insert("confirm_large".into(), true.into());
            self.spawn(app.clone(), job, options);
        }
    }

    pub fn quick_cancel(&self, id: &str) {
        if let Some(job) = self.waiting.lock().unwrap().remove(id) {
            if job
                .admitted_id
                .is_some_and(|fid| apply::matches_identity(&job.current, fid))
            {
                if let Some(at) = job.renamed_at.filter(|_| !job.from_menu) {
                    apply::wait_for_file_notice(&job.current, at);
                }
                if apply::restore_verified(&job) && !job.from_menu {
                    shell_notice::file_restored(&job.current, &job.original);
                }
            }
            self.in_flight.lock().unwrap().remove(&job.current);
        }
    }

    fn all_folders(&self, extra: &[String]) -> Vec<PathBuf> {
        let mut all = watch::user_folders();
        all.extend(extra.iter().map(PathBuf::from));
        all
    }

    /// The user's own folders first, then the ones they added.
    pub fn folders(&self, app: &AppHandle) -> Vec<WatchedFolder> {
        let builtin = watch::user_folders().into_iter().map(|p| WatchedFolder {
            path: p.to_string_lossy().into_owned(),
            builtin: true,
        });
        let extra = crate::settings::load(app)
            .rename_folders
            .into_iter()
            .map(|path| WatchedFolder {
                path,
                builtin: false,
            });
        builtin.chain(extra).collect()
    }

    /// Saves the added folders and starts or stops listening right away.
    pub fn set_extra_folders(&self, app: &AppHandle, extra: Vec<String>) -> Result<(), String> {
        let saved = crate::settings::save(app, |s| s.rename_folders = extra.clone());
        if let Some(watcher) = self.watcher.get() {
            watcher.set_folders(&self.all_folders(&extra));
        }
        saved
    }

    fn known(&self, id: &str) -> bool {
        KNOWN_EXTENSIONS.get().is_some_and(|k| k.contains(id))
    }

    fn can_route(&self, source: &str, target: &str) -> bool {
        let family = self.registry.resolve_family(source);
        // Write-only formats (PDF, AVIF) do not resolve to a family by name.
        if family != self.registry.resolve_family(target) && !self.writes(&family, target) {
            return false;
        }
        let Some(handler) = self.registry.get_handler(&family) else {
            return false;
        };
        let descriptors = handler.format_descriptors();
        let (Some(input), Some(output)) = (
            find_descriptor(descriptors, source),
            find_descriptor(descriptors, target),
        ) else {
            return false;
        };
        output.can_write
            && handler
                .capabilities()
                .iter()
                .any(|c| c.input.0 == input.id && c.outputs.iter().any(|o| o.0 == output.id))
    }

    /// Right-click menu: each format ZeroConverter reads (every spelling) and
    /// the formats it can become.
    pub fn menu_entries(&self) -> Vec<crate::context_menu::Entry> {
        let mut entries = Vec::new();
        for family in [AssetFamily::Image, AssetFamily::Audio, AssetFamily::Video] {
            let Some(handler) = self.registry.get_handler(&family) else {
                continue;
            };
            let descriptors = handler.format_descriptors();
            for source in descriptors.iter().filter(|d| d.can_read) {
                let mut targets: Vec<String> =
                    [AssetFamily::Image, AssetFamily::Audio, AssetFamily::Video]
                        .iter()
                        .filter_map(|f| self.registry.get_handler(f))
                        .flat_map(|h| h.format_descriptors().iter())
                        .filter(|t| {
                            t.can_write && t.canonical_extension != source.canonical_extension
                        })
                        .filter(|t| {
                            self.can_route(source.canonical_extension, t.canonical_extension)
                        })
                        .map(|t| t.canonical_extension.to_string())
                        .collect();
                targets.sort();
                targets.dedup();
                for ext in std::iter::once(source.canonical_extension)
                    .chain(source.aliases.iter().copied())
                {
                    entries.push((ext.to_string(), targets.clone()));
                }
            }
        }
        entries
    }

    /// Every extension a menu may sit on, for removing it.
    pub fn known_extensions(&self) -> Vec<String> {
        KNOWN_EXTENSIONS
            .get()
            .map(|k| k.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// From the right-click menu: convert a copy next to the file, keep the
    /// original. `photo.webp` already there → `photo (2).webp`.
    pub fn convert_copy(self: &Arc<Self>, app: &AppHandle, file: PathBuf, target: String) {
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let target = crate::core::sniff::canonical_ext(&target);
        let Some(content) = crate::core::sniff::sniff_file(&file) else {
            notify(
                app,
                Tone::Failure,
                &tr("rename.cantConvert", &[]),
                &tr("rename.notReadable", &[("name", &name)]),
            );
            return;
        };
        if !self.can_route(content, &target) {
            notify(
                app,
                Tone::Warning,
                &tr("rename.cantConvert", &[]),
                &tr(
                    "rename.unsupported",
                    &[
                        ("source", &content.to_uppercase()),
                        ("target", &target.to_uppercase()),
                    ],
                ),
            );
            return;
        }
        let Some(admitted_id) = apply::capture_identity(&file) else {
            notify(
                app,
                Tone::Failure,
                &tr("rename.cantConvert", &[]),
                &tr("rename.notReadable", &[("name", &name)]),
            );
            return;
        };
        let job = Job {
            current: file.clone(),
            original: file.clone(),
            final_path: apply::free_name(&file.with_extension(&target)),
            source: self.canonical(content),
            target,
            keep_original: true,
            max_size: None,
            from_menu: true,
            renamed_at: None,
            admitted_id: Some(admitted_id),
        };
        if !self.in_flight.lock().unwrap().insert(job.current.clone()) {
            return;
        }
        if large_input(&file).is_some() {
            let id = uuid::Uuid::new_v4().simple().to_string();
            self.waiting.lock().unwrap().insert(id.clone(), job);
            open_quick_window(app, &id);
        } else {
            self.spawn(app.clone(), job, QuickOptions::default());
        }
    }

    /// Can any converter write this format?
    fn writable(&self, target: &str) -> bool {
        [AssetFamily::Image, AssetFamily::Audio, AssetFamily::Video]
            .iter()
            .any(|f| self.writes(f, target))
    }

    fn writes(&self, family: &AssetFamily, target: &str) -> bool {
        self.registry
            .get_handler(family)
            .and_then(|h| find_descriptor(h.format_descriptors(), target).map(|d| d.can_write))
            .unwrap_or(false)
    }
}

/// Whether the original file stays on disk: setting XOR `&`.
fn keep_original_for(setting: bool, has_keep_mark: bool) -> bool {
    setting ^ has_keep_mark
}

fn success_card_body(keep_original: bool, done: &apply::Done) -> String {
    let bin = tr(
        if keep_original {
            "rename.keptOriginal"
        } else if done.original_recycled {
            "rename.inBin"
        } else {
            "rename.notBinned"
        },
        &[],
    );
    join_sentences(&[
        tr("rename.converted", &[("time", &took(done.elapsed))]),
        bin,
    ])
}

/// Runs on the watcher threads: only names ending in a format we know (or
/// that plus marks like `+`, `&`, `@1080`) go any further.
fn quick_filter(name: &str) -> bool {
    let (name, _) = rule::split_modifiers(name);
    let Some(ext) = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
    else {
        return false;
    };
    KNOWN_EXTENSIONS
        .get()
        .is_some_and(|k| k.contains(&ext.to_ascii_lowercase()))
}

/// Puts back the name a file had before the rename when `now` is still the
/// same file (or still unreadable when `allow_unreadable` is true). Never
/// overwrites a file that took `before` meanwhile; returns whether `now` was
/// actually moved back to `before`.
fn restore(
    now: &Path,
    before: &Path,
    event_at: std::time::Instant,
    id: Option<apply::FileIdentity>,
    allow_unreadable: bool,
) -> bool {
    let matches_now = || match id {
        Some(fid) => apply::matches_identity(now, fid),
        None => allow_unreadable && apply::capture_identity(now).is_none(),
    };
    if !matches_now() {
        return false;
    }
    if now != before {
        apply::wait_for_file_notice(now, event_at);
    }
    if !matches_now() {
        return false;
    }
    let ok = zero_fs::rename_no_replace(now, before).is_ok();
    if ok {
        shell_notice::file_restored(now, before);
    }
    ok
}

/// Renames a folder back, retrying while Explorer still holds it. Returns
/// where the folder is now.
fn rename_back(now: &Path, before: &Path) -> PathBuf {
    // Explorer must finish its own rename first, or it shows "The file or
    // folder does not exist". It says so with a shell notice; a rename made
    // elsewhere sends none, and then the full wait passes.
    let started = std::time::Instant::now();
    let heard = shell_notice::wait_for_folder_rename(now, EXPLORER_RENAME_WAIT);
    debug(&format!(
        "folder notice heard={heard} after {:?}",
        started.elapsed()
    ));
    if heard {
        // Explorer still looks at the new name once more right after its notice.
        std::thread::sleep(EXPLORER_SETTLE);
    }
    for attempt in 0..40 {
        match zero_fs::rename_no_replace(now, before) {
            Ok(()) => {
                debug(&format!(
                    "folder renamed back after {:?} (attempt {attempt})",
                    started.elapsed()
                ));
                // Explorer overlooks a change on disk this soon after its own.
                shell_notice::folder_renamed(now, before);
                return before.to_path_buf();
            }
            // Something else has the old name: leave the folder as it is.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => break,
            Err(e) => {
                if attempt == 0 {
                    debug(&format!("folder rename back failed: {e}"));
                }
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
        }
    }
    debug(&format!(
        "folder keeps its new name after {:?}",
        started.elapsed()
    ));
    now.to_path_buf()
}

/// Every file below `root`, skipping hidden folders (ZeroConverter's own
/// scratch folders among them) and engine or build folders. Stops just past
/// the folder limit.
fn folder_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                if !hidden && !watch::skipped(&format!("\\{}\\", rel.display())) {
                    stack.push(path);
                }
            } else if kind.is_file() && !hidden {
                files.push(path);
                if files.len() > MAX_FOLDER_FILES {
                    return files;
                }
            }
        }
    }
    files
}

/// The files a folder conversion takes on, and the ones it leaves out.
struct FolderPlan {
    jobs: Vec<Job>,
    /// The new name is taken (by a file, or by another file of the folder).
    taken: usize,
    /// Over 100 MB: converted one by one only.
    large: usize,
    /// Whether originals stay on disk (setting XOR `&`).
    keep: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct FolderCounts {
    done: usize,
    failed: usize,
    not_binned: usize,
    stopped: usize,
    /// Left as they were because FFmpeg is not there yet.
    no_ffmpeg: usize,
}

/// The conversion needs FFmpeg and it is not installed: the user is sent to
/// the download instead of being shown an error.
fn needs_ffmpeg(e: &AppError) -> bool {
    matches!(e, AppError::Execute(m) if m == crate::core::handlers::video::execute::FFMPEG_MISSING)
}

/// The card shown when a folder conversion ends.
fn folder_summary(
    c: &FolderCounts,
    large: usize,
    taken: usize,
    keep: bool,
    elapsed: std::time::Duration,
) -> (Tone, String) {
    let FolderCounts {
        done,
        failed,
        not_binned,
        stopped,
        no_ffmpeg,
    } = *c;
    let n = |v: usize| v.to_string();
    let mut parts = vec![tr_n(
        "rename.folderConverted",
        done as u64,
        &[("time", &took(elapsed))],
    )];
    if done > 0 {
        if keep {
            parts.push(tr("rename.originalsKept", &[]));
        } else if not_binned == 0 {
            parts.push(tr("rename.originalsBin", &[]));
        }
        if !keep && not_binned > 0 {
            parts.push(tr_n("rename.notBinnedMany", not_binned as u64, &[]));
        }
    }
    if failed > 0 {
        parts.push(tr("rename.folderFailed", &[("count", &n(failed))]));
    }
    if stopped > 0 {
        parts.push(tr("rename.folderStopped", &[("count", &n(stopped))]));
    }
    if no_ffmpeg > 0 {
        parts.push(tr("rename.folderNoFfmpeg", &[("count", &n(no_ffmpeg))]));
    }
    if large > 0 {
        parts.push(tr("rename.folderLarge", &[("count", &n(large))]));
    }
    if taken > 0 {
        parts.push(tr("rename.folderTaken", &[("count", &n(taken))]));
    }
    let body = join_sentences(&parts);
    let tone = if failed > 0 || no_ffmpeg > 0 {
        Tone::Warning
    } else {
        Tone::Success
    };
    (tone, body)
}

fn took(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else {
        format!("{secs:.1} s")
    }
}

/// Retries transient read failures briefly (Explorer or antivirus may hold
/// the file right after a rename) without sleeping once reading succeeds.
fn retry_read<T, E>(
    attempts: usize,
    delay: std::time::Duration,
    mut read: impl FnMut() -> Result<T, E>,
    mut sleep: impl FnMut(std::time::Duration),
) -> Option<T> {
    let attempts = attempts.max(1);
    for attempt in 0..attempts {
        if let Ok(value) = read() {
            return Some(value);
        }
        if attempt + 1 < attempts && !delay.is_zero() {
            sleep(delay);
        }
    }
    None
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn decision_notice(
    decision: &Decision,
    old: &Path,
    new: &Path,
    restored: bool,
) -> Option<(String, String)> {
    match decision {
        Decision::Unsupported { source, target } => {
            let body = if restored {
                tr(
                    "rename.unsupportedBack",
                    &[
                        ("source", &source.to_uppercase()),
                        ("target", &target.to_uppercase()),
                    ],
                )
            } else {
                join_sentences(&[
                    tr(
                        "rename.unsupported",
                        &[
                            ("source", &source.to_uppercase()),
                            ("target", &target.to_uppercase()),
                        ],
                    ),
                    tr("rename.stillNamed", &[("name", &base_name(new))]),
                ])
            };
            Some((tr("rename.cantConvert", &[]), body))
        }
        Decision::Unreadable => {
            let body = if restored {
                tr("rename.unreadableBack", &[("name", &base_name(old))])
            } else {
                join_sentences(&[
                    tr("rename.notReadable", &[("name", &base_name(new))]),
                    tr("rename.stillNamed", &[("name", &base_name(new))]),
                ])
            };
            Some((tr("rename.cantConvert", &[]), body))
        }
        Decision::Mismatch { expected, actual } => {
            let body = if restored {
                let name = base_name(old);
                let expected = expected.to_uppercase();
                match actual {
                    Some(actual) => tr(
                        "rename.mismatchBack",
                        &[
                            ("name", &name),
                            ("expected", &expected),
                            ("actual", &actual.to_uppercase()),
                            ("ext", &actual.to_ascii_lowercase()),
                        ],
                    ),
                    None => tr(
                        "rename.mismatchUnknownBack",
                        &[("name", &name), ("expected", &expected)],
                    ),
                }
            } else {
                join_sentences(&[
                    tr("rename.notReadable", &[("name", &base_name(new))]),
                    tr("rename.stillNamed", &[("name", &base_name(new))]),
                ])
            };
            Some((tr("rename.cantConvert", &[]), body))
        }
        Decision::Ignore | Decision::Restore | Decision::Convert { .. } => None,
    }
}

fn notify(app: &AppHandle, tone: Tone, title: &str, body: &str) {
    crate::receipt::show(
        app,
        crate::receipt::Receipt {
            tone,
            title: title.into(),
            body: body.into(),
        },
    );
}

fn open_quick_window(app: &AppHandle, id: &str) {
    let label = format!("quick-{id}");
    let url = tauri::WebviewUrl::App(format!("index.html#/quick/{id}").into());
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let _ = tauri::WebviewWindowBuilder::new(&app, label, url)
            .title("ZeroConverter")
            // The page then fits the height to its options (QuickConvert).
            .inner_size(380.0, 340.0)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            // In the taskbar and Alt+Tab, so a window that lost focus can
            // be found again; the file waits under its `+` name until then.
            .skip_taskbar(false)
            .center()
            .focused(true)
            .build();
    });
}

fn format_debug_line(elapsed: std::time::Duration, msg: &str) -> String {
    format!("[{:>8.3}s] [rename-convert] {msg}", elapsed.as_secs_f64())
}

/// Diagnostics, only when ZC_DEBUG_RENAME is set.
pub fn debug(msg: &str) {
    if std::env::var_os("ZC_DEBUG_RENAME").is_some() {
        let start = DEBUG_START.get_or_init(std::time::Instant::now);
        eprintln!("{}", format_debug_line(start.elapsed(), msg));
    }
}

/// Lets the main window's settings read the state.
pub fn state(app: &AppHandle) -> Arc<RenameConvert> {
    app.state::<Arc<RenameConvert>>().inner().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_line_prefixes_elapsed_time() {
        assert_eq!(
            format_debug_line(std::time::Duration::from_millis(12_345), "step"),
            "[  12.345s] [rename-convert] step"
        );
        assert_eq!(
            format_debug_line(std::time::Duration::from_millis(5), "start"),
            "[   0.005s] [rename-convert] start"
        );
    }

    #[test]
    fn slots_let_a_few_through_and_give_up_on_request() {
        let slots = Slots::new(2);
        let a = slots.take(|| false);
        let b = slots.take(|| false);
        assert!(a.is_some() && b.is_some());
        assert!(slots.take(|| true).is_none(), "full: gives up");
        drop(a);
        assert!(slots.take(|| true).is_some(), "freed: taken");
    }

    /// `Holiday` → `Holiday.webp` where every file that can convert does: the
    /// card is a success, and two files wanting the same new name (`a.png`,
    /// `a.jpg`) count as skipped, not as a failure (they once showed a
    /// warning card although nothing went wrong).
    #[test]
    fn missing_ffmpeg_points_to_the_download_not_a_failure() {
        use crate::core::handlers::video::execute::FFMPEG_MISSING;
        // The text is checked in English, whatever language this PC runs.
        crate::i18n::set("en");
        assert!(needs_ffmpeg(&AppError::Execute(FFMPEG_MISSING.into())));
        assert!(!needs_ffmpeg(&AppError::Execute("decoder error".into())));
        let counts = FolderCounts {
            done: 2,
            no_ffmpeg: 1,
            ..Default::default()
        };
        let (tone, body) = folder_summary(&counts, 0, 0, false, std::time::Duration::ZERO);
        assert_eq!(tone, Tone::Warning);
        assert!(body.contains("need FFmpeg"), "{body}");
        assert!(!body.contains("could not be converted"), "{body}");
    }

    #[test]
    fn a_folder_that_converts_fully_reports_success() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let picture = |name: &str, format: image::ImageFormat| {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            image::RgbImage::from_pixel(16, 8, image::Rgb([10, 120, 200]))
                .save_with_format(&path, format)
                .unwrap();
        };
        picture("a.png", image::ImageFormat::Png);
        picture("a.jpg", image::ImageFormat::Jpeg);
        picture("b.png", image::ImageFormat::Png);
        picture("sub/c.jpg", image::ImageFormat::Jpeg);
        picture("d.png", image::ImageFormat::Png);
        picture("d.webp", image::ImageFormat::WebP);
        std::fs::write(root.join("notes.txt"), b"not a picture").unwrap();

        let rc = RenameConvert::new(Arc::new(ConverterRegistry::standard()));
        let plan = rule::FolderConvert {
            target: "webp".into(),
            modifiers: rule::Modifiers::default(),
        };
        let FolderPlan {
            jobs,
            taken,
            large,
            keep,
        } = rc.plan_folder(root, &plan).unwrap();
        assert_eq!((jobs.len(), taken, large, keep), (3, 2, 0, false));
        let counts = rc.run_folder(&jobs, keep);
        assert_eq!(
            counts,
            FolderCounts {
                done: 3,
                ..Default::default()
            }
        );
        let (tone, body) = folder_summary(&counts, large, taken, keep, std::time::Duration::ZERO);
        assert_eq!(tone, Tone::Success, "{body}");
        for name in ["a.webp", "b.webp", "sub/c.webp", "d.webp"] {
            assert_eq!(
                crate::core::sniff::sniff_file(&root.join(name)),
                Some("webp"),
                "{name}"
            );
        }
        // No scratch folder is left behind.
        assert!(std::fs::read_dir(root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".zc-")));
    }

    #[test]
    fn folder_files_skips_hidden_and_engine_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in [
            "a.png",
            "sub/b.png",
            ".zc-123/c.png",
            "Game/Library/d.png",
            ".hidden.png",
        ] {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let mut names: Vec<String> = folder_files(root)
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        names.sort();
        assert_eq!(names, ["a.png", "sub/b.png"]);
    }

    #[test]
    fn keep_original_xor_covers_all_four_states() {
        assert!(!keep_original_for(false, false));
        assert!(keep_original_for(false, true));
        assert!(keep_original_for(true, false));
        assert!(!keep_original_for(true, true));
    }

    #[test]
    fn keep_original_setting_applies_xor_to_rename_jobs_and_success_card() {
        crate::i18n::set("en");
        let rc = RenameConvert::new(Arc::new(ConverterRegistry::standard()));

        for (setting, has_keep_mark, expected_keep) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            rc.keep_original.store(setting, Ordering::Relaxed);
            let dir = tempfile::tempdir().unwrap();
            let old = dir.path().join("photo.png");
            let new = dir.path().join(if has_keep_mark {
                "photo.webp&"
            } else {
                "photo.webp"
            });
            image::RgbaImage::from_pixel(16, 8, image::Rgba([10, 120, 200, 255]))
                .save_with_format(&new, image::ImageFormat::Png)
                .unwrap();

            let Decision::Convert {
                source,
                target,
                modifiers,
                final_path,
            } = rule::decide(
                &old,
                &new,
                crate::core::sniff::sniff_file(&new)
                    .map_or(rule::Content::Unknown, rule::Content::Known),
                |id| rc.known(id),
                |s, t| rc.can_route(s, t),
            )
            else {
                panic!("expected Decision::Convert for setting={setting}, &={has_keep_mark}");
            };

            let job = rc.build_rename_job(
                &old,
                &new,
                &source,
                target,
                modifiers,
                &final_path,
                None,
                apply::capture_identity(&new),
            );
            assert_eq!(
                job.keep_original, expected_keep,
                "setting={setting}, &={has_keep_mark}"
            );
            let done = apply::run(&job, &rc.registry, &QuickOptions::default()).unwrap();
            assert_eq!(old.exists(), expected_keep);
            assert_eq!(
                done.kept_original.as_deref(),
                expected_keep.then_some(old.as_path())
            );
            assert!(dir.path().join("photo.webp").exists());
            let body = success_card_body(job.keep_original, &done);
            if expected_keep {
                assert!(body.contains("Original kept."), "{body}");
            } else {
                assert!(body.contains("Original is in the Recycle Bin."), "{body}");
            }
        }

        // Same-format resize with setting ON keeps the original under its name
        // and places the resized output at a free name unless `&` flips it.
        rc.keep_original.store(true, Ordering::Relaxed);
        for (has_keep_mark, expected_keep) in [(false, true), (true, false)] {
            let dir = tempfile::tempdir().unwrap();
            let old = dir.path().join("photo.png");
            let new = dir.path().join(if has_keep_mark {
                "photo.png@16&"
            } else {
                "photo.png@16"
            });
            image::RgbaImage::from_pixel(32, 16, image::Rgba([10, 120, 200, 255]))
                .save_with_format(&new, image::ImageFormat::Png)
                .unwrap();

            let Decision::Convert {
                source,
                target,
                modifiers,
                final_path,
            } = rule::decide(
                &old,
                &new,
                crate::core::sniff::sniff_file(&new)
                    .map_or(rule::Content::Unknown, rule::Content::Known),
                |id| rc.known(id),
                |s, t| rc.can_route(s, t),
            )
            else {
                panic!("expected Decision::Convert for same-format resize");
            };

            let job = rc.build_rename_job(
                &old,
                &new,
                &source,
                target,
                modifiers,
                &final_path,
                None,
                apply::capture_identity(&new),
            );
            assert_eq!(job.keep_original, expected_keep);
            let done = apply::run(&job, &rc.registry, &QuickOptions::default()).unwrap();
            if expected_keep {
                assert_ne!(done.final_path, old);
                assert_eq!(image::image_dimensions(&old).unwrap(), (32, 16));
                assert_eq!(image::image_dimensions(&done.final_path).unwrap(), (16, 8));
            } else {
                assert_eq!(done.final_path, old);
                assert_eq!(image::image_dimensions(&old).unwrap(), (16, 8));
            }
        }
    }

    #[test]
    fn keep_original_setting_applies_xor_to_folder_conversions() {
        crate::i18n::set("en");
        let rc = RenameConvert::new(Arc::new(ConverterRegistry::standard()));

        for (setting, has_keep_mark, expected_keep) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            rc.keep_original.store(setting, Ordering::Relaxed);
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("Holiday");
            std::fs::create_dir(&folder).unwrap();
            let src_img = folder.join("a.png");
            image::RgbImage::from_pixel(16, 8, image::Rgb([10, 120, 200]))
                .save_with_format(&src_img, image::ImageFormat::Png)
                .unwrap();

            let renamed = dir.path().join(if has_keep_mark {
                "Holiday.webp&"
            } else {
                "Holiday.webp"
            });
            let plan =
                rule::decide_folder(&folder, &renamed, |id| rc.writable(id)).expect("folder plan");
            let FolderPlan {
                jobs,
                taken,
                large,
                keep,
            } = rc.plan_folder(&folder, &plan).unwrap();
            assert_eq!(keep, expected_keep);
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].keep_original, expected_keep);

            let counts = rc.run_folder(&jobs, keep);
            assert_eq!(counts.done, 1);
            assert_eq!(src_img.exists(), expected_keep);
            assert!(folder.join("a.webp").exists());
            let (tone, body) =
                folder_summary(&counts, large, taken, keep, std::time::Duration::ZERO);
            assert_eq!(tone, Tone::Success, "{body}");
            if expected_keep {
                assert!(body.contains("Originals kept."), "{body}");
            } else {
                assert!(body.contains("Originals are in the Recycle Bin."), "{body}");
            }
        }
    }

    #[test]
    fn retry_read_stops_immediately_on_success_and_bounds_retries() {
        let delay = std::time::Duration::from_millis(50);

        // Immediate Ok(None) (readable, unrecognised bytes) does not retry or sleep.
        let mut reads = 0;
        let mut sleeps = 0;
        let out = retry_read(
            SNIFF_ATTEMPTS,
            delay,
            || -> Result<Option<&'static str>, ()> {
                reads += 1;
                Ok(None)
            },
            |_| sleeps += 1,
        );
        assert_eq!(out, Some(None));
        assert_eq!((reads, sleeps), (1, 0));

        // Transient read errors succeed on attempt 3 after 2 sleeps.
        let mut reads = 0;
        let mut sleeps = Vec::new();
        let out = retry_read(
            SNIFF_ATTEMPTS,
            delay,
            || -> Result<Option<&'static str>, ()> {
                reads += 1;
                if reads < 3 {
                    Err(())
                } else {
                    Ok(Some("m4a"))
                }
            },
            |d| sleeps.push(d),
        );
        assert_eq!(out, Some(Some("m4a")));
        assert_eq!(reads, 3);
        assert_eq!(sleeps, vec![delay, delay]);

        // Persistent read errors give up after SNIFF_ATTEMPTS without a trailing sleep.
        let mut reads = 0;
        let mut sleeps = Vec::new();
        let out = retry_read(
            SNIFF_ATTEMPTS,
            delay,
            || -> Result<Option<&'static str>, ()> {
                reads += 1;
                Err(())
            },
            |d| sleeps.push(d),
        );
        assert_eq!(out, None);
        assert_eq!(reads, SNIFF_ATTEMPTS);
        assert_eq!(sleeps.len(), SNIFF_ATTEMPTS - 1);
        assert_eq!(
            sleeps.into_iter().sum::<std::time::Duration>(),
            std::time::Duration::from_millis(300)
        );
    }

    #[test]
    fn warning_cards_explain_unreadable_and_mismatched_renames() {
        crate::i18n::set("en");
        let old = Path::new("C:/Music/song.m4a");
        let new = Path::new("C:/Music/song.mp3");

        assert_eq!(
            decision_notice(&Decision::Unreadable, old, new, true),
            Some((
                "Can't convert this one".into(),
                "Couldn't read song.m4a. It may be in use by another program or not downloaded from the cloud yet. The name was put back.".into(),
            ))
        );
        assert_eq!(
            decision_notice(
                &Decision::Mismatch {
                    expected: "m4a".into(),
                    actual: Some("webm".into()),
                },
                old,
                new,
                true,
            ),
            Some((
                "Can't convert this one".into(),
                "song.m4a isn't really a M4A file (it looks like WEBM). Rename it to .webm first, or convert it from the app window. The name was put back.".into(),
            ))
        );
        assert_eq!(
            decision_notice(
                &Decision::Mismatch {
                    expected: "m4a".into(),
                    actual: None,
                },
                old,
                new,
                true,
            ),
            Some((
                "Can't convert this one".into(),
                "song.m4a isn't really a M4A file. The name was put back.".into(),
            ))
        );
        // When restoration could not happen (e.g. old name taken), never claim the name went back.
        let (_, not_restored) = decision_notice(&Decision::Unreadable, old, new, false).unwrap();
        assert!(!not_restored.contains("put back"), "{not_restored}");
        assert!(not_restored.contains("song.mp3"), "{not_restored}");
    }

    #[test]
    fn quick_cancel_leaves_substitute_file_untouched() {
        let rc = RenameConvert::new(Arc::new(ConverterRegistry::standard()));
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("photo.png");
        let cur = dir.path().join("photo.webp+");
        std::fs::write(&cur, b"\x89PNG\r\n\x1a\nfirst").unwrap();
        let admitted = apply::capture_identity(&cur);

        std::fs::remove_file(&cur).unwrap();
        std::fs::write(&cur, b"foreign replacement").unwrap();

        let job = rc.build_rename_job(
            &old,
            &cur,
            "png",
            "webp".into(),
            rule::Modifiers {
                ask_options: true,
                ..Default::default()
            },
            &dir.path().join("photo.webp"),
            None,
            admitted,
        );
        rc.in_flight.lock().unwrap().insert(cur.clone());
        rc.waiting.lock().unwrap().insert("id1".into(), job);

        rc.quick_cancel("id1");
        assert_eq!(std::fs::read(&cur).unwrap(), b"foreign replacement");
        assert!(!old.exists());
        assert!(!rc.in_flight.lock().unwrap().contains(&cur));
    }
}
