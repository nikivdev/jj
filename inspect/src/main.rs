use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    queue,
    style::{Attribute, Color, Print, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use serde::Deserialize;
use unicode_width::UnicodeWidthStr;

const EMPTY_TREE_HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone)]
struct CommitItem {
    id: String,
    summary: String,
}

#[derive(Debug, Clone)]
struct FileItem {
    status: String,
    path: String,
    original_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Stack,
    Queue,
}

#[derive(Debug)]
struct AppState {
    repo: PathBuf,
    base_revset: String,
    mode: ViewMode,
    commits: Vec<CommitItem>,
    commit_index: usize,
    file_index: usize,
    diff_scroll: usize,
    files_cache: HashMap<String, Vec<FileItem>>,
    diff_cache: HashMap<String, Vec<String>>,
    status: String,
    input_mode: bool,
    input_buffer: String,
}

#[derive(Debug, Deserialize)]
struct CommitQueueEntry {
    created_at: String,
    commit_sha: String,
    message: String,
    #[serde(default)]
    review_bookmark: Option<String>,
}

fn main() -> Result<()> {
    let (repo, base_revset, limit, mode) = parse_args()?;
    let mut app = AppState::new(repo, base_revset, mode)?;
    app.refresh(limit)?;
    if app.commits.is_empty() {
        println!("No commits found.");
        return Ok(());
    }
    run_tui(&mut app)?;
    Ok(())
}

fn parse_args() -> Result<(PathBuf, String, usize, ViewMode)> {
    let mut repo = std::env::current_dir().context("resolve cwd")?;
    let mut base_revset = String::new();
    let mut limit = 50usize;
    let mut mode = ViewMode::Stack;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => {
                let value = args.next().context("--repo requires a path")?;
                repo = PathBuf::from(value);
            }
            "--base" => {
                base_revset = args.next().context("--base requires a revset")?;
            }
            "--limit" => {
                let value = args.next().context("--limit requires a number")?;
                limit = value.parse::<usize>().context("invalid --limit")?;
            }
            "--queue" => {
                mode = ViewMode::Queue;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => {}
        }
    }

    if base_revset.is_empty() && mode == ViewMode::Stack {
        base_revset = resolve_default_base(&repo)?;
    }

    Ok((repo, base_revset, limit, mode))
}

fn print_help() {
    println!(
        "jj-inspect - stack/queue review TUI\n\n\
Usage:\n  jj-inspect [--repo <path>] [--base <revset>] [--limit <n>] [--queue]\n\n\
Keys:\n  j/k, Down/Up  Move file\n  [ / ]         Prev/Next commit\n  PgDn/PgUp     Scroll diff\n  g/G           Top/Bottom file\n  r             Refresh\n  Enter         Open full diff\n  :             Command mode\n  A             Approve commit (queue mode)\n  q             Quit\n\n\
Modes:\n  --queue        Show Flow commit-queue entries (from .ai/internal/commit-queue)\n"
    );
}

impl AppState {
    fn new(repo: PathBuf, base_revset: String, mode: ViewMode) -> Result<Self> {
        Ok(Self {
            repo,
            base_revset,
            mode,
            commits: Vec::new(),
            commit_index: 0,
            file_index: 0,
            diff_scroll: 0,
            files_cache: HashMap::new(),
            diff_cache: HashMap::new(),
            status: String::new(),
            input_mode: false,
            input_buffer: String::new(),
        })
    }

    fn refresh(&mut self, limit: usize) -> Result<()> {
        let commits = match self.mode {
            ViewMode::Stack => load_stack_commits(&self.repo, &self.base_revset, limit)?,
            ViewMode::Queue => load_queue_commits(&self.repo, limit)?,
        };
        self.commits = commits;
        self.commit_index = 0;
        self.file_index = 0;
        self.diff_scroll = 0;
        self.files_cache.clear();
        self.diff_cache.clear();
        self.status = match self.mode {
            ViewMode::Stack => format!("stack base: {}", self.base_revset),
            ViewMode::Queue => "queue: flow commit-queue".to_string(),
        };
        Ok(())
    }

    fn selected_commit(&self) -> Option<&CommitItem> {
        self.commits.get(self.commit_index)
    }

    fn selected_file(&self) -> Option<&FileItem> {
        self.files_for_selected_commit()
            .and_then(|files| files.get(self.file_index))
    }

    fn files_for_selected_commit(&self) -> Option<&Vec<FileItem>> {
        let commit_id = self.selected_commit()?.id.as_str();
        self.files_cache.get(commit_id)
    }

    fn ensure_files_loaded(&mut self) -> Result<()> {
        let commit_id = match self.selected_commit() {
            Some(commit) => commit.id.clone(),
            None => return Ok(()),
        };
        if !self.files_cache.contains_key(&commit_id) {
            let files = match self.mode {
                ViewMode::Stack => load_stack_files(&self.repo, &commit_id)?,
                ViewMode::Queue => load_queue_files(&self.repo, &commit_id)?,
            };
            self.files_cache.insert(commit_id, files);
            self.file_index = 0;
        }
        Ok(())
    }

    fn move_file_selection(&mut self, delta: isize) {
        let Some(files) = self.files_for_selected_commit() else {
            return;
        };
        if files.is_empty() {
            return;
        }
        let len = files.len() as isize;
        let next = (self.file_index as isize + delta).clamp(0, len - 1) as usize;
        if next != self.file_index {
            self.file_index = next;
            self.diff_scroll = 0;
        }
    }

    fn jump_file_top(&mut self) {
        self.file_index = 0;
        self.diff_scroll = 0;
    }

    fn jump_file_bottom(&mut self) {
        let len = self.files_for_selected_commit().map(|v| v.len()).unwrap_or(0);
        if len > 0 {
            self.file_index = len - 1;
            self.diff_scroll = 0;
        }
    }

    fn move_commit(&mut self, delta: isize) {
        if self.commits.is_empty() {
            return;
        }
        let len = self.commits.len() as isize;
        let next = (self.commit_index as isize + delta).clamp(0, len - 1) as usize;
        if next != self.commit_index {
            self.commit_index = next;
            self.file_index = 0;
            self.diff_scroll = 0;
        }
    }

    fn scroll_diff(&mut self, delta: isize) {
        let next = self.diff_scroll as isize + delta;
        self.diff_scroll = next.max(0) as usize;
    }

    fn diff_lines(&mut self, commit_id: &str, file: Option<&FileItem>) -> Result<Vec<String>> {
        let key = match file {
            Some(item) => format!("{}::{}", commit_id, item.path),
            None => commit_id.to_string(),
        };
        if !self.diff_cache.contains_key(&key) {
            let diff = match self.mode {
                ViewMode::Stack => load_stack_diff(&self.repo, commit_id, file)?,
                ViewMode::Queue => load_queue_diff(&self.repo, commit_id, file)?,
            };
            self.diff_cache.insert(key.clone(), diff);
        }
        Ok(self
            .diff_cache
            .get(&key)
            .map(|v| v.clone())
            .unwrap_or_default())
    }

    fn run_command(&mut self, command: &str) {
        if command.trim().is_empty() {
            return;
        }
        match run_shell(&self.repo, command) {
            Ok(output) => {
                let line = output.lines().next().unwrap_or("ok");
                self.status = format!("> {}", line);
            }
            Err(err) => {
                self.status = format!("command failed: {}", err);
            }
        }
    }

    fn approve_selected(&mut self) {
        if self.mode != ViewMode::Queue {
            self.status = "approve only works in queue mode".to_string();
            return;
        }
        let Some(commit) = self.selected_commit() else {
            return;
        };
        let cmd = format!("f commit-queue approve {}", commit.id);
        self.run_command(&cmd);
    }
}

fn run_tui(app: &mut AppState) -> Result<()> {
    let mut stdout = init_terminal()?;

    let tick_rate = Duration::from_millis(120);
    loop {
        app.ensure_files_loaded()?;
        draw_ui(app, &mut stdout)?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                if handle_key(app, key)? {
                    break;
                }
            }
        }
    }

    restore_terminal(&mut stdout)?;
    Ok(())
}

fn init_terminal() -> Result<Stdout> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    Ok(stdout)
}

fn restore_terminal(stdout: &mut Stdout) -> Result<()> {
    execute!(stdout, LeaveAlternateScreen, cursor::Show)?;
    terminal::disable_raw_mode()?;
    Ok(())
}

fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    if app.input_mode {
        match key.code {
            KeyCode::Esc => {
                app.input_mode = false;
                app.input_buffer.clear();
            }
            KeyCode::Enter => {
                let cmd = app.input_buffer.clone();
                app.input_mode = false;
                app.input_buffer.clear();
                app.run_command(&cmd);
            }
            KeyCode::Backspace => {
                app.input_buffer.pop();
            }
            KeyCode::Char(ch) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.input_buffer.push(ch);
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('j') | KeyCode::Down => app.move_file_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_file_selection(-1),
        KeyCode::Char('[') => app.move_commit(-1),
        KeyCode::Char(']') => app.move_commit(1),
        KeyCode::PageDown => app.scroll_diff(10),
        KeyCode::PageUp => app.scroll_diff(-10),
        KeyCode::Char('g') => app.jump_file_top(),
        KeyCode::Char('G') => app.jump_file_bottom(),
        KeyCode::Char('r') => app.refresh(50)?,
        KeyCode::Char(':') => {
            app.input_mode = true;
            app.input_buffer.clear();
        }
        KeyCode::Char('A') => app.approve_selected(),
        KeyCode::Enter => {
            if let Some(commit) = app.selected_commit() {
                let file = app.selected_file();
                open_full_diff(&app.repo, &commit.id, file, app.mode)?;
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        _ => {}
    }
    Ok(false)
}

fn draw_ui(app: &mut AppState, stdout: &mut Stdout) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let list_width = ((cols as f32) * 0.38).max(28.0) as u16;
    let diff_width = cols.saturating_sub(list_width + 1);
    let body_rows = rows.saturating_sub(3) as usize;

    queue!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    let header_left = match app.selected_commit() {
        Some(commit) => format!("{} {}", short_id(&commit.id), commit.summary),
        None => "(no commit)".to_string(),
    };
    queue!(
        stdout,
        SetAttribute(Attribute::Bold),
        Print(truncate_to_width(&header_left, list_width as usize)),
        cursor::MoveTo(list_width + 1, 0),
        Print("Diff"),
        SetAttribute(Attribute::Reset)
    )?;

    let commit_id = app.selected_commit().map(|c| c.id.clone()).unwrap_or_default();
    let files = app.files_for_selected_commit().cloned().unwrap_or_default();
    let selected = app.file_index;
    let diff_scroll = app.diff_scroll;
    let selected_file = files.get(selected);
    let diff_lines = app.diff_lines(&commit_id, selected_file).unwrap_or_default();

    for row in 0..body_rows {
        let y = (row + 1) as u16;
        let mut left_line = String::new();
        if let Some(file) = files.get(row) {
            let prefix = if row == selected { "▸ " } else { "  " };
            let display_path = if file.status.starts_with('R') {
                if let Some(original) = file.original_path.as_ref() {
                    format!("{} -> {}", original, file.path)
                } else {
                    file.path.clone()
                }
            } else {
                file.path.clone()
            };
            left_line = format!("{}{} {}", prefix, file.status, display_path);
        }

        let diff_index = diff_scroll + row;
        let mut diff_line = if diff_index < diff_lines.len() {
            diff_lines[diff_index].replace('\t', "    ")
        } else {
            String::new()
        };

        left_line = truncate_to_width(&left_line, list_width as usize);
        diff_line = truncate_to_width(&diff_line, diff_width as usize);

        queue!(stdout, cursor::MoveTo(0, y))?;
        if row == selected {
            queue!(stdout, SetForegroundColor(Color::Yellow))?;
        }
        queue!(stdout, Print(left_line), SetForegroundColor(Color::Reset))?;
        queue!(stdout, cursor::MoveTo(list_width + 1, y), Print(diff_line))?;
    }

    let status = format!("{}  |  {} files  |  commit {}/{}", app.status, files.len(), app.commit_index + 1, app.commits.len());
    let status_line = truncate_to_width(&status, cols as usize);
    queue!(stdout, cursor::MoveTo(0, rows.saturating_sub(2)), Print(status_line))?;

    if app.input_mode {
        let prompt = format!(":{}", app.input_buffer);
        let line = truncate_to_width(&prompt, cols as usize);
        queue!(stdout, cursor::MoveTo(0, rows.saturating_sub(1)), Print(line))?;
    } else {
        let hint = "[j/k] files  [/] commits  [A] approve  [:] command  [Enter] diff";
        let line = truncate_to_width(hint, cols as usize);
        queue!(stdout, cursor::MoveTo(0, rows.saturating_sub(1)), Print(line))?;
    }

    stdout.flush()?;
    Ok(())
}

fn truncate_to_width(value: &str, width: usize) -> String {
    let mut out = String::new();
    let mut current = 0;
    for ch in value.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if current + w > width {
            break;
        }
        out.push(ch);
        current += w;
    }
    if out.len() < value.len() && width > 1 {
        if current + 1 <= width {
            out.push('…');
        }
    }
    out
}

fn load_stack_commits(repo: &Path, base_revset: &str, limit: usize) -> Result<Vec<CommitItem>> {
    let revset = format!("ancestors(@) & ~ancestors({})", base_revset);
    let template = "commit_id ++ \"\\t\" ++ description.first_line()";
    let output = run_jj(repo, &["log", "-r", &revset, "--no-graph", "-T", template])?;
    let mut commits = Vec::new();
    for line in output.lines() {
        let mut parts = line.splitn(2, '\t');
        let id = parts.next().unwrap_or("").trim().to_string();
        if id.is_empty() {
            continue;
        }
        let summary = parts.next().unwrap_or("").trim().to_string();
        commits.push(CommitItem { id, summary });
        if commits.len() >= limit {
            break;
        }
    }
    Ok(commits)
}

fn load_stack_files(repo: &Path, commit_id: &str) -> Result<Vec<FileItem>> {
    let output = run_jj(repo, &["diff", "-r", commit_id, "--name-status", "--color", "never"])?;
    Ok(parse_name_status(&output))
}

fn load_queue_commits(repo: &Path, limit: usize) -> Result<Vec<CommitItem>> {
    let queue_dir = repo.join(".ai").join("internal").join("commit-queue");
    if !queue_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&queue_dir).context("read queue dir")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if let Ok(parsed) = serde_json::from_str::<CommitQueueEntry>(&content) {
            entries.push(parsed);
        }
    }
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    let mut commits = Vec::new();
    for entry in entries.into_iter().take(limit) {
        let mut summary = entry.message.lines().next().unwrap_or("").to_string();
        if summary.is_empty() {
            summary = "(no message)".to_string();
        }
        if let Some(bookmark) = entry.review_bookmark.as_deref() {
            summary = format!("{}  [{}]", summary, bookmark);
        }
        commits.push(CommitItem {
            id: entry.commit_sha,
            summary,
        });
    }
    Ok(commits)
}

fn load_queue_files(repo: &Path, commit_id: &str) -> Result<Vec<FileItem>> {
    let output = run_git(repo, &["diff-tree", "--root", "--no-commit-id", "--name-status", "-r", "-M", commit_id])?;
    Ok(parse_name_status(&output))
}

fn load_stack_diff(repo: &Path, commit_id: &str, file: Option<&FileItem>) -> Result<Vec<String>> {
    let mut args = vec!["diff", "-r", commit_id, "--color", "never"];
    if let Some(item) = file {
        args.push("--");
        args.push(&item.path);
    }
    let output = run_jj(repo, &args)?;
    Ok(output.lines().map(|s| s.to_string()).collect())
}

fn load_queue_diff(repo: &Path, commit_id: &str, file: Option<&FileItem>) -> Result<Vec<String>> {
    let parent = match run_git(repo, &["rev-parse", &format!("{}^", commit_id)]) {
        Ok(parent) if !parent.trim().is_empty() => parent.trim().to_string(),
        _ => EMPTY_TREE_HASH.to_string(),
    };
    let mut args = vec!["diff", &parent, commit_id, "--color", "never"];
    if let Some(item) = file {
        args.push("--");
        args.push(&item.path);
    }
    let output = run_git(repo, &args)?;
    Ok(output.lines().map(|s| s.to_string()).collect())
}

fn open_full_diff(
    repo: &Path,
    commit_id: &str,
    file: Option<&FileItem>,
    mode: ViewMode,
) -> Result<()> {
    terminal::disable_raw_mode().ok();
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -R".to_string());
    let cmd = match mode {
        ViewMode::Stack => {
            if let Some(item) = file {
                format!(
                    "jj diff -r {} --color always -- {}",
                    commit_id,
                    shell_quote(&item.path)
                )
            } else {
                format!("jj diff -r {} --color always", commit_id)
            }
        }
        ViewMode::Queue => {
            let parent = match run_git(repo, &["rev-parse", &format!("{}^", commit_id)]) {
                Ok(parent) if !parent.trim().is_empty() => parent.trim().to_string(),
                _ => EMPTY_TREE_HASH.to_string(),
            };
            if let Some(item) = file {
                format!(
                    "git diff --color=always {} {} -- {}",
                    parent,
                    commit_id,
                    shell_quote(&item.path)
                )
            } else {
                format!("git diff --color=always {} {}", parent, commit_id)
            }
        }
    };
    let cmd = format!("{} | {}", cmd, pager);
    let status = Command::new("sh")
        .args(["-c", &cmd])
        .current_dir(repo)
        .status()
        .context("run pager")?;
    if !status.success() {
        println!("Failed to open pager.");
    }
    terminal::enable_raw_mode().ok();
    Ok(())
}

fn run_shell(repo: &Path, command: &str) -> Result<String> {
    let output = Command::new("sh")
        .args(["-c", command])
        .current_dir(repo)
        .output()
        .context("run shell")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_jj(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("jj")
        .args(args)
        .current_dir(repo)
        .output()
        .context("run jj")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .context("run git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_name_status(output: &str) -> Vec<FileItem> {
    let mut items = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }
        let status = parts[0].trim().to_string();
        if status.starts_with('R') && parts.len() >= 3 {
            items.push(FileItem {
                status: status.clone(),
                path: parts[2].to_string(),
                original_path: Some(parts[1].to_string()),
            });
        } else if parts.len() >= 2 {
            items.push(FileItem {
                status: status.clone(),
                path: parts[1].to_string(),
                original_path: None,
            });
        }
    }
    items
}

fn short_id(commit_id: &str) -> String {
    if commit_id.len() <= 8 {
        commit_id.to_string()
    } else {
        commit_id[..8].to_string()
    }
}

fn resolve_default_base(repo: &Path) -> Result<String> {
    let candidates = ["trunk()", "main@origin", "master@origin", "main", "master"];
    for candidate in candidates {
        if jj_revset_exists(repo, candidate) {
            return Ok(candidate.to_string());
        }
    }
    Ok("root()".to_string())
}

fn jj_revset_exists(repo: &Path, revset: &str) -> bool {
    let output = Command::new("jj")
        .args(["log", "-r", revset, "--no-graph", "-T", "commit_id"])
        .current_dir(repo)
        .output();
    if let Ok(output) = output {
        output.status.success() && !output.stdout.is_empty()
    } else {
        false
    }
}

fn shell_quote(value: &str) -> String {
    let mut out = String::new();
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
