//! Requests the host sends and the replies the guest returns.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Host to guest: one request with a correlation id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    /// Run `op`; answer with a [`Reply`] carrying the same `id`.
    Call {
        /// Unique per connection.
        id: u64,
        /// Wall-clock budget; the guest gives up (and kills child processes) after it.
        deadline_ms: u64,
        /// What to do.
        op: Op,
    },
    /// Stop work on an earlier call (the caller went away). The guest still replies.
    Cancel {
        /// The call to stop.
        id: u64,
    },
}

/// Guest to host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    /// The [`Envelope::Call`] this answers.
    pub id: u64,
    /// The result, or a typed failure.
    pub result: Result<Value, GuestError>,
}

/// A typed failure from the guest. `message` is written for the agent to read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct GuestError {
    /// Stable category.
    pub code: GuestErrorCode,
    /// What happened, in plain words.
    pub message: String,
    /// What to do about it, when there is something to do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl GuestError {
    /// Build an error with no hint.
    pub fn new(code: GuestErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }
    /// Attach a hint.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Stable failure categories.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GuestErrorCode {
    /// Arguments were missing or out of range.
    InvalidArgs,
    /// A file, app, window, element or shortcut does not exist.
    NotFound,
    /// macOS privacy settings (TCC) block this: Accessibility, Screen Recording,
    /// Automation or Full Disk Access.
    PermissionDenied,
    /// The deadline passed. Side effects may have happened.
    Timeout,
    /// The call was cancelled by the host.
    Cancelled,
    /// A script or command ran and failed.
    Failed,
    /// The result was larger than allowed.
    TooLarge,
    /// The element snapshot this ref came from is stale; read the UI again.
    StaleRef,
    /// This guest cannot do that (older agent or missing tool).
    Unsupported,
    /// The guest is shutting down or overloaded.
    Busy,
}

/// Every operation the guest understands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Op {
    /// Liveness. Returns `{}`.
    Ping,
    /// Returns [`GuestInfo`].
    Info,
    /// Run a shell command as the signed-in user. Returns [`ExecResult`].
    Exec(Exec),
    /// Run AppleScript or JavaScript for Automation. Returns [`ScriptResult`].
    Script(Script),
    /// Capture the screen, a display, a region or one app's front window.
    /// Returns [`Screenshot`].
    Screenshot(ScreenshotArgs),
    /// Read the accessibility tree of an app's windows. Returns [`UiTree`].
    UiTree(UiTreeArgs),
    /// Act on an element from the last [`UiTree`]. Returns `{}` or a value.
    UiAction(UiActionArgs),
    /// Find elements by text/role without reading the whole tree. Returns [`UiTree`].
    UiFind(UiFindArgs),
    /// Mouse input at screen points. Returns `{}`.
    Pointer(PointerArgs),
    /// Keyboard input. Returns `{}`.
    Keyboard(KeyboardArgs),
    /// App management.
    Apps(AppsOp),
    /// Open a URL (Safari is the default browser).
    OpenUrl(OpenUrlArgs),
    /// File operations, as the signed-in user.
    Files(FilesOp),
    /// Clipboard.
    Clipboard(ClipboardOp),
    /// Shortcuts app.
    Shortcuts(ShortcutsOp),
    /// Safari, driven through its scripting interface.
    Safari(SafariOp),
    /// Post a user notification banner in the Mac.
    Notify(NotifyArgs),
    /// Replace the agent with a new version, sent in chunks over this link.
    Update(UpdateOp),
    /// MCP servers that apps on the Mac provide, bridged for the assistant.
    Mcp(McpOp),
}

impl Op {
    /// Short stable name for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Ping => "ping",
            Op::Info => "info",
            Op::Exec(_) => "exec",
            Op::Script(_) => "script",
            Op::Screenshot(_) => "screenshot",
            Op::UiTree(_) => "ui_tree",
            Op::UiAction(_) => "ui_action",
            Op::UiFind(_) => "ui_find",
            Op::Pointer(_) => "pointer",
            Op::Keyboard(_) => "keyboard",
            Op::Apps(_) => "apps",
            Op::OpenUrl(_) => "open_url",
            Op::Files(_) => "files",
            Op::Clipboard(_) => "clipboard",
            Op::Shortcuts(_) => "shortcuts",
            Op::Safari(_) => "safari",
            Op::Notify(_) => "notify",
            Op::Update(_) => "update",
            Op::Mcp(_) => "mcp",
        }
    }
}

/// What the guest knows about its Mac.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GuestInfo {
    /// `sw_vers -productVersion`.
    pub os_version: String,
    /// `sw_vers -buildVersion`.
    #[serde(default)]
    pub os_build: String,
    /// Computer name.
    pub hostname: String,
    /// The signed-in user the agent runs as.
    pub user: String,
    /// Seconds since boot.
    #[serde(default)]
    pub uptime_s: u64,
    /// Main display in points, and its backing scale.
    #[serde(default)]
    pub displays: Vec<Display>,
    /// Privacy permissions the agent holds.
    #[serde(default)]
    pub permissions: Permissions,
    /// Free space on the boot volume.
    #[serde(default)]
    pub disk_free_bytes: u64,
    /// The app in front, if any.
    #[serde(default)]
    pub frontmost_app: Option<String>,
    /// Default web browser bundle id.
    #[serde(default)]
    pub default_browser: Option<String>,
    /// Ops this agent supports (by [`Op::name`]), so the host can adapt to older agents.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// A display in points.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Display {
    /// 1-based, as `screencapture -D` numbers them.
    pub index: u32,
    /// Width in points.
    pub width: u32,
    /// Height in points.
    pub height: u32,
    /// Backing scale factor (2 on Retina).
    pub scale: f64,
    /// Is this the main display.
    pub main: bool,
}

/// macOS privacy grants. `None` means the agent could not tell.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Permissions {
    /// Needed for UI tree, clicks and typing.
    pub accessibility: Option<bool>,
    /// Needed for screenshots that show other apps' windows.
    pub screen_recording: Option<bool>,
    /// Needed to read Mail, Messages, Safari data and other protected folders.
    pub full_disk_access: Option<bool>,
}

/// Run a command through `/bin/zsh -lc`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exec {
    /// The command line.
    pub command: String,
    /// Working directory; `~` expands. Defaults to the home folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Text written to stdin, then closed.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Combined cap for stdout and stderr; the rest is dropped and `truncated` set.
    #[serde(default = "default_output_cap")]
    pub max_output_bytes: usize,
}

fn default_output_cap() -> usize {
    256 * 1024
}

// Matches the serde defaults, so `Exec { command, ..Exec::default() }` gets the same
// output cap as a request that omits the field (a derived Default would give 0).
impl Default for Exec {
    fn default() -> Self {
        Self {
            command: String::new(),
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            max_output_bytes: default_output_cap(),
        }
    }
}

/// Result of [`Op::Exec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Exit status; `None` when killed by a signal.
    pub exit_code: Option<i32>,
    /// Lossy UTF-8.
    pub stdout: String,
    /// Lossy UTF-8.
    pub stderr: String,
    /// Output went over the cap.
    pub truncated: bool,
    /// The deadline killed it.
    pub timed_out: bool,
    /// Wall time.
    pub duration_ms: u64,
}

/// Script language.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptLanguage {
    /// AppleScript.
    Applescript,
    /// JavaScript for Automation.
    Javascript,
}

/// Run a script with `osascript`. Arguments are passed as `argv`, never spliced into
/// the source, so values can't change what the script does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    /// Language.
    pub language: ScriptLanguage,
    /// Source.
    pub source: String,
    /// Passed to `on run argv` / `function run(argv)`.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Result of [`Op::Script`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    /// What the script returned, as text. JSON when the script returned JSON.
    pub output: String,
}

/// A rectangle in screen points, origin top-left of the main display.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Image encoding.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// Lossless; best for text.
    Png,
    /// Smaller; the default.
    #[default]
    Jpeg,
}

/// What to capture. With nothing set, the whole main display.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenshotArgs {
    /// 1-based display index.
    #[serde(default)]
    pub display: Option<u32>,
    /// A region in points.
    #[serde(default)]
    pub region: Option<Rect>,
    /// Capture only this app's front window (name or bundle id), e.g. "Safari".
    #[serde(default)]
    pub app: Option<String>,
    /// Downscale so the image is at most this wide in pixels. Default 1440.
    #[serde(default)]
    pub max_width: Option<u32>,
    /// Encoding.
    #[serde(default)]
    pub format: ImageFormat,
    /// Draw the mouse pointer.
    #[serde(default)]
    pub cursor: bool,
}

/// Result of [`Op::Screenshot`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    /// Base64 image bytes.
    pub data: String,
    /// `image/png` or `image/jpeg`.
    pub mime: String,
    /// Pixel width of the returned image.
    pub width: u32,
    /// Pixel height of the returned image.
    pub height: u32,
    /// The captured area in screen points, so the agent can map pixels to clicks.
    pub area: Rect,
    /// Points per image pixel horizontally (`area.width / width`).
    pub points_per_pixel: f64,
}

/// Read an accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTreeArgs {
    /// App name or bundle id; defaults to the frontmost app.
    #[serde(default)]
    pub app: Option<String>,
    /// Depth limit below each window. Default 12.
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// Node limit. Default 400.
    #[serde(default)]
    pub max_nodes: Option<u32>,
    /// Include the menu bar.
    #[serde(default)]
    pub include_menus: bool,
}

/// Find elements matching a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiFindArgs {
    /// App name or bundle id; defaults to the frontmost app.
    #[serde(default)]
    pub app: Option<String>,
    /// Case-insensitive substring of title, value, description or help.
    #[serde(default)]
    pub text: Option<String>,
    /// Exact AX role, e.g. `AXButton`.
    #[serde(default)]
    pub role: Option<String>,
    /// Result limit. Default 25.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One accessibility element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiNode {
    /// Stable within its snapshot, e.g. `e12`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Depth below the app (windows are 1).
    pub depth: u32,
    /// AX role.
    pub role: String,
    /// AX subrole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    /// Title or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Current value, as text, truncated to 500 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// AX description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Frame in screen points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    /// Can be acted on.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Has keyboard focus.
    #[serde(default)]
    pub focused: bool,
    /// AX actions, e.g. `AXPress`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

fn yes() -> bool {
    true
}

/// Result of [`Op::UiTree`] and [`Op::UiFind`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiTree {
    /// Snapshot id the refs belong to. Refs from older snapshots are [`GuestErrorCode::StaleRef`].
    pub snapshot: u64,
    /// The app read.
    pub app: String,
    /// Elements in document order.
    pub nodes: Vec<UiNode>,
    /// Hit the node limit.
    pub truncated: bool,
}

/// Act on an element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiActionArgs {
    /// A ref from the latest [`UiTree`].
    #[serde(rename = "ref")]
    pub reference: String,
    /// What to do.
    pub action: UiAction,
}

/// Element actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiAction {
    /// `AXPress`, falling back to a click at the element's center.
    Press,
    /// Give it keyboard focus.
    Focus,
    /// Set its value (text fields, checkboxes, sliders).
    SetValue {
        /// New value.
        value: String,
    },
    /// Open its context menu.
    ShowMenu,
    /// Bring its window to the front.
    Raise,
    /// Scroll it into view.
    ScrollIntoView,
    /// Any other AX action by name.
    Perform {
        /// e.g. `AXIncrement`.
        name: String,
    },
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// Primary.
    #[default]
    Left,
    /// Secondary.
    Right,
    /// Middle.
    Middle,
}

/// Mouse input. Coordinates are screen points.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PointerArgs {
    /// Click (count 2 for a double-click).
    Click {
        /// Left.
        x: f64,
        /// Top.
        y: f64,
        /// Which button.
        #[serde(default)]
        button: Button,
        /// Click count.
        #[serde(default = "one")]
        count: u32,
        /// Held modifiers: `cmd`, `shift`, `option`, `ctrl`.
        #[serde(default)]
        modifiers: Vec<String>,
    },
    /// Move without clicking (hover).
    Move {
        /// Left.
        x: f64,
        /// Top.
        y: f64,
    },
    /// Press at one point and release at another.
    Drag {
        /// Start left.
        from_x: f64,
        /// Start top.
        from_y: f64,
        /// End left.
        to_x: f64,
        /// End top.
        to_y: f64,
    },
    /// Scroll wheel at a point. Positive `dy` scrolls content down.
    Scroll {
        /// Left.
        x: f64,
        /// Top.
        y: f64,
        /// Horizontal lines.
        #[serde(default)]
        dx: i32,
        /// Vertical lines.
        #[serde(default)]
        dy: i32,
    },
}

fn one() -> u32 {
    1
}

/// Keyboard input: text, a chord, or both (text first).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyboardArgs {
    /// Literal text to type.
    #[serde(default)]
    pub text: Option<String>,
    /// A chord like `cmd+shift+t`, `return`, `escape`, `tab`, `down`, `f5`.
    #[serde(default)]
    pub keys: Option<String>,
    /// Bring this app to the front first.
    #[serde(default)]
    pub app: Option<String>,
}

/// App management.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AppsOp {
    /// Installed apps (`/Applications`, `~/Applications`, `/System/Applications`).
    /// Returns `Vec<AppInfo>`.
    List {
        /// Only running apps.
        #[serde(default)]
        running_only: bool,
    },
    /// Launch (or bring forward) an app, optionally opening files or a URL with it.
    Open {
        /// Name or bundle id.
        app: String,
        /// Paths or URLs to open with it.
        #[serde(default)]
        open: Vec<String>,
        /// Launch without bringing to the front.
        #[serde(default)]
        background: bool,
    },
    /// Quit an app. `force` kills it.
    Quit {
        /// Name or bundle id.
        app: String,
        /// Kill instead of asking.
        #[serde(default)]
        force: bool,
    },
    /// Windows of an app (or every app). Returns `Vec<WindowInfo>`.
    Windows {
        /// Name or bundle id.
        #[serde(default)]
        app: Option<String>,
    },
    /// Install an app. Returns [`AppInfo`].
    Install {
        /// Where from.
        source: InstallSource,
    },
    /// The app's scripting dictionary, condensed. Returns [`AppDictionary`].
    Dictionary {
        /// Name or bundle id.
        app: String,
    },
    /// Move an app to the Trash. Returns `{}`.
    Uninstall {
        /// Name or bundle id.
        app: String,
    },
}

/// Where to install an app from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallSource {
    /// An https URL to a `.dmg`, `.zip` or `.pkg`.
    Url {
        /// https only.
        url: String,
        /// Expected SHA-256 (hex). Strongly recommended.
        #[serde(default)]
        sha256: Option<String>,
    },
    /// Mac App Store, by numeric id (needs `mas` and a signed-in Apple Account).
    AppStore {
        /// App Store id.
        id: u64,
    },
    /// Homebrew cask token.
    Brew {
        /// e.g. `visual-studio-code`.
        cask: String,
    },
}

/// An installed or running app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppInfo {
    /// Display name.
    pub name: String,
    /// Bundle id.
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// Path to the `.app`.
    #[serde(default)]
    pub path: Option<String>,
    /// `CFBundleShortVersionString`.
    #[serde(default)]
    pub version: Option<String>,
    /// Currently running.
    #[serde(default)]
    pub running: bool,
    /// Has an AppleScript dictionary.
    #[serde(default)]
    pub scriptable: bool,
}

/// A window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    /// Owning app.
    pub app: String,
    /// Title.
    pub title: String,
    /// Frame in points.
    pub frame: Option<Rect>,
    /// Minimized.
    #[serde(default)]
    pub minimized: bool,
    /// Frontmost window of the frontmost app.
    #[serde(default)]
    pub focused: bool,
}

/// A condensed scripting dictionary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AppDictionary {
    /// App name.
    pub app: String,
    /// Suites with their commands and classes.
    pub suites: Vec<Suite>,
}

/// One dictionary suite.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Suite {
    /// Name.
    pub name: String,
    /// Commands: `name(params) -> result: description`.
    pub commands: Vec<String>,
    /// Classes: `name { properties; elements }: description`.
    pub classes: Vec<String>,
}

/// Open a URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenUrlArgs {
    /// Any URL scheme the Mac handles.
    pub url: String,
    /// App to open it with; defaults to the handler (Safari for web).
    #[serde(default)]
    pub app: Option<String>,
}

/// File operations. Paths may start with `~`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FilesOp {
    /// Returns [`FileContent`].
    Read {
        /// Path.
        path: String,
        /// Byte offset.
        #[serde(default)]
        offset: u64,
        /// Byte limit. Default 1 MiB, at most 32 MiB.
        #[serde(default)]
        length: Option<u64>,
    },
    /// Atomic write (temp file then rename). Returns [`FileEntry`].
    Write {
        /// Path.
        path: String,
        /// UTF-8 text, or base64 when `base64` is set.
        content: String,
        /// `content` is base64.
        #[serde(default)]
        base64: bool,
        /// Append instead of replace.
        #[serde(default)]
        append: bool,
        /// Refuse to replace an existing file.
        #[serde(default)]
        create_only: bool,
        /// Create parent folders.
        #[serde(default)]
        parents: bool,
    },
    /// Returns `Vec<FileEntry>`.
    List {
        /// Folder.
        path: String,
        /// Recurse this many levels. Default 0.
        #[serde(default)]
        depth: u32,
        /// Include dotfiles.
        #[serde(default)]
        hidden: bool,
        /// Entry cap. Default 500.
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Returns [`FileEntry`].
    Stat {
        /// Path.
        path: String,
    },
    /// Returns [`FileEntry`] for the destination.
    Move {
        /// Source.
        from: String,
        /// Destination.
        to: String,
        /// Replace an existing destination (the old one goes to the Trash).
        #[serde(default)]
        overwrite: bool,
    },
    /// Returns [`FileEntry`] for the destination.
    Copy {
        /// Source.
        from: String,
        /// Destination.
        to: String,
        /// Replace an existing destination (the old one goes to the Trash).
        #[serde(default)]
        overwrite: bool,
    },
    /// Returns [`FileEntry`].
    Mkdir {
        /// Path; parents are created.
        path: String,
    },
    /// Move to the Trash (recoverable). Returns `{ "trashed": path }`.
    Trash {
        /// Path.
        path: String,
    },
    /// Spotlight search. Returns `Vec<FileEntry>`.
    Search {
        /// Spotlight query or plain words.
        query: String,
        /// Folder to search in.
        #[serde(default)]
        scope: Option<String>,
        /// Result cap. Default 50.
        #[serde(default)]
        limit: Option<u32>,
    },
}

/// A file or folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// Absolute path.
    pub path: String,
    /// `file`, `dir`, `symlink` or `other`.
    pub kind: String,
    /// Bytes (files only).
    pub size: u64,
    /// RFC 3339.
    #[serde(default)]
    pub modified: Option<String>,
}

/// Result of [`FilesOp::Read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    /// Absolute path.
    pub path: String,
    /// UTF-8 text when the bytes are valid UTF-8, else base64.
    pub content: String,
    /// `content` is base64.
    pub base64: bool,
    /// Total file size.
    pub size: u64,
    /// More bytes follow `offset + returned`.
    pub truncated: bool,
    /// Guessed MIME type.
    pub mime: String,
}

/// Clipboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClipboardOp {
    /// Returns `{ "text": String | null }`.
    Get,
    /// Returns `{}`.
    Set {
        /// Plain text.
        text: String,
    },
}

/// Shortcuts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ShortcutsOp {
    /// Returns `Vec<String>`.
    List,
    /// Returns `{ "output": String }`.
    Run {
        /// Shortcut name.
        name: String,
        /// Text input.
        #[serde(default)]
        input: Option<String>,
    },
}

/// Safari. Page-content ops need Develop > Allow JavaScript from Apple Events, which
/// the golden image turns on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SafariOp {
    /// Every window and tab. Returns `Vec<SafariTab>`.
    Tabs,
    /// Open a URL. Returns [`SafariTab`].
    Open {
        /// URL.
        url: String,
        /// In a new tab (default) or replacing the current one.
        #[serde(default = "yes")]
        new_tab: bool,
    },
    /// Make a tab current. Returns [`SafariTab`].
    Activate {
        /// From [`SafariOp::Tabs`].
        window: u32,
        /// From [`SafariOp::Tabs`].
        tab: u32,
    },
    /// Close a tab. Returns `{}`.
    Close {
        /// Window index.
        window: u32,
        /// Tab index.
        tab: u32,
    },
    /// Readable text, title, URL and links of the current (or given) tab.
    /// Returns [`SafariPage`].
    Read {
        /// Window index; default front.
        #[serde(default)]
        window: Option<u32>,
        /// Tab index; default current.
        #[serde(default)]
        tab: Option<u32>,
        /// Character cap. Default 60000.
        #[serde(default)]
        max_chars: Option<u32>,
    },
    /// Run JavaScript in the page; returns `{ "result": <JSON> }`.
    Eval {
        /// Window index; default front.
        #[serde(default)]
        window: Option<u32>,
        /// Tab index; default current.
        #[serde(default)]
        tab: Option<u32>,
        /// An expression; its JSON-serializable value is returned.
        script: String,
    },
    /// Interact with the page by CSS selector. Returns `{ "matched": n }`.
    Dom {
        /// Window index; default front.
        #[serde(default)]
        window: Option<u32>,
        /// Tab index; default current.
        #[serde(default)]
        tab: Option<u32>,
        /// CSS selector.
        selector: String,
        /// What to do with the first match.
        #[serde(rename = "do")]
        act: DomAction,
    },
    /// Back, forward or reload. Returns [`SafariTab`].
    Navigate {
        /// `back`, `forward` or `reload`.
        direction: String,
    },
    /// Wait until the current tab finishes loading. Returns [`SafariTab`].
    WaitLoaded,
}

/// What to do with an element in a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomAction {
    /// Click it.
    Click,
    /// Set an input's value and fire input/change events.
    Fill {
        /// Value.
        value: String,
    },
    /// Choose an option in a select by value or label.
    Select {
        /// Value or label.
        value: String,
    },
    /// Submit its form.
    Submit,
    /// Scroll it into view.
    Scroll,
    /// Return its text and attributes.
    Inspect,
}

/// A Safari tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafariTab {
    /// 1-based window index.
    pub window: u32,
    /// 1-based tab index.
    pub tab: u32,
    /// Title.
    pub title: String,
    /// URL.
    pub url: String,
    /// The current tab of the front window.
    #[serde(default)]
    pub current: bool,
}

/// Readable page content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafariPage {
    /// Title.
    pub title: String,
    /// URL.
    pub url: String,
    /// Main text, whitespace-collapsed.
    pub text: String,
    /// Up to 200 links: `[text, href]`.
    pub links: Vec<(String, String)>,
    /// Text was cut at `max_chars`.
    pub truncated: bool,
}

/// A notification banner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyArgs {
    /// Title.
    pub title: String,
    /// Body.
    pub message: String,
}

/// Agent self-update, streamed over the authenticated link so it works the same for VMs
/// and remote Macs and needs no extra network path or credentials.
///
/// The guest keeps versions side by side and a supervisor that rolls back to the previous
/// version if a new one fails to come up, so a bad update can't strand a Mac.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum UpdateOp {
    /// Start a transfer, discarding any unfinished one. Returns `{}`.
    Begin {
        /// Version string the new binary reports with `--version`.
        version: String,
        /// SHA-256 of the whole binary (hex).
        sha256: String,
        /// Total bytes.
        size: u64,
    },
    /// The next piece, in order. Returns `{ "received": bytes so far }`.
    Chunk {
        /// Byte offset; must equal the bytes received so far.
        offset: u64,
        /// Base64 bytes, at most [`UPDATE_CHUNK`] decoded.
        data: String,
    },
    /// Verify size, hash and `--version`, install, then restart into it. Returns
    /// `{ "installed": version, "restarting": true }`; the link drops right after and
    /// the host reconnects to the new agent.
    Commit,
    /// Drop an unfinished transfer. Returns `{}`.
    Abort,
}

/// Largest decoded chunk in [`UpdateOp::Chunk`].
pub const UPDATE_CHUNK: usize = 4 * 1024 * 1024;

/// MCP servers on the Mac. The host names a server only by the id [`McpOp::Discover`]
/// gave it; the guest looks the command or address up again itself, so nothing the
/// host sends is ever run as a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum McpOp {
    /// Find servers without starting any of them. Returns `Vec<McpServer>`.
    Discover,
    /// Start the server if needed and list its tools. Returns `Vec<McpTool>`.
    ListTools {
        /// [`McpServer::id`].
        server: String,
        /// The fingerprint the owner approved. The guest refuses if the server it finds
        /// now has another one, so a config change between looks can't slip through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_fingerprint: Option<String>,
    },
    /// Call one tool. Returns the server's `CallToolResult` as it sent it.
    CallTool {
        /// [`McpServer::id`].
        server: String,
        /// The tool's own name, as the server lists it.
        tool: String,
        /// Arguments for the tool.
        #[serde(default)]
        arguments: serde_json::Map<String, Value>,
        /// As in [`McpOp::ListTools`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_fingerprint: Option<String>,
    },
}

/// How the guest reaches a server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// A command the guest starts, speaking JSON-RPC over stdin and stdout.
    Stdio,
    /// Streamable HTTP on the Mac's own loopback address.
    Http,
}

/// An MCP server found on the Mac.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServer {
    /// Stable id: lowercase letters, digits and `_`, at most 24 characters.
    pub id: String,
    /// Display name, like "Xcode" or "things".
    pub name: String,
    /// The app it belongs to, when it comes with one.
    #[serde(default)]
    pub app: Option<String>,
    /// That app's bundle id.
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// How it's reached.
    pub transport: McpTransport,
    /// The command (without environment values) or the loopback address, for the owner
    /// to recognise. Never contains secrets from config files.
    pub detail: String,
    /// Where it was found: `app` for a server that ships with an app, otherwise the
    /// assistant whose config lists it (`Claude`, `Cursor`, `VS Code`, `Claude Code`).
    pub source: String,
    /// SHA-256 (hex) of everything that decides what runs: source, command, args, env
    /// (names and values) or URL. The host keeps it with the owner's approval and treats
    /// the server as off once it changes. Empty from agents that predate it.
    #[serde(default)]
    pub fingerprint: String,
}

/// A tool an MCP server offers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    /// Name, as the server lists it.
    pub name: String,
    /// `title` or `annotations.title`.
    #[serde(default)]
    pub title: Option<String>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for the arguments (`{"type": "object"}` when the server gave none).
    pub input_schema: Value,
    /// `annotations.readOnlyHint`.
    #[serde(default)]
    pub read_only: bool,
    /// `annotations.destructiveHint` (MCP's default for tools that aren't read-only is true).
    #[serde(default)]
    pub destructive: bool,
}
