# Familiar tools

What an assistant can do with a Mac. Each tool belongs to a permission group the owner
sets to Allow, Ask or Never on the Mac's Permissions page. Groups set to Never hide their
tools entirely.

| Group (default) | Tools |
| --- | --- |
| See the screen (Allow) | `mac_status`, `mac_screenshot` (screen, display, region or one app's window, any time), `mac_ui_tree`, `mac_ui_find`, `mac_windows`, `mac_apps`, `app_dictionary`, `mac_wait` |
| Use the Mac (Allow) | `mac_click` (pixels from the last screenshot, points, or an element ref), `mac_ui_action`, `mac_type`, `mac_key`, `mac_scroll`, `mac_drag`, `mac_hover`, `mac_open_app`, `mac_quit_app`, `mac_open_url`, `mac_notify`, `shortcuts_list`, `shortcuts_run` |
| Safari (Allow) | `safari_tabs`, `safari_open`, `safari_read`, `safari_screenshot`, `safari_dom`, `safari_eval`, `safari_navigate`, `safari_activate_tab`, `safari_close_tab` |
| Read files (Allow) | `file_read`, `file_list`, `file_stat`, `file_search` |
| Change files (Allow) | `file_write`, `file_move`, `file_copy`, `file_mkdir`, `file_trash` (to the Trash, never deleted) |
| Terminal and scripts (Allow) | `mac_shell`, `mac_script` (AppleScript or JXA; values passed as arguments) |
| Clipboard (Allow) | `clipboard_read`, `clipboard_write` |
| Checkpoints (Allow) | `mac_checkpoint`, `mac_checkpoints` |
| Install and restore (Ask) | `app_install` (https download, App Store, Homebrew), `app_uninstall`, `mac_restore_checkpoint`, `mac_restart` |
| Send to other people (Ask) | every pack tool that sends, e.g. `mail_send`, `messages_send` |

App packs (each its own group, Allow by default; toggle per app on the Apps page):

| Pack | Tools |
| --- | --- |
| Calendar | `calendar_calendars`, `calendar_events` (expands repeating events), `calendar_event`, `calendar_create`, `calendar_update`, `calendar_delete` |
| Reminders | `reminders_lists`, `reminders_items`, `reminders_create`, `reminders_update`, `reminders_complete`, `reminders_delete` |
| Notes | `notes_folders`, `notes_search`, `notes_read`, `notes_create`, `notes_append`, `notes_move` |
| Mail | `mail_accounts`, `mail_mailboxes`, `mail_messages`, `mail_search`, `mail_read`, `mail_send`, `mail_draft`, `mail_move`, `mail_mark` |
| Messages | `messages_chats`, `messages_recent` (needs Full Disk Access), `messages_send` |
| Contacts | `contacts_search`, `contacts_read`, `contacts_create` |
| Music | `music_now_playing`, `music_control`, `music_search_library`, `music_play` |
| Finder | `finder_selection`, `finder_reveal`, `finder_open_folder`, `finder_tags`, `finder_set_tags` |
| Photos | `photos_albums`, `photos_search`, `photos_export` |

Owners can add their own packs (docs/home/PACKS.md), and any scriptable app works through
`app_dictionary` + `mac_script`. Apps the owner switches off on the Apps page can't be
targeted by name.

## App servers

Some apps on the Mac run their own MCP server. Familiar bridges them, so their tools sit
next to the ones above as `mcp_<server>_<tool>` (for example `mcp_xcode_BuildProject`;
names that need cleaning up or shortening end in a short hash). The tool's own schema,
description and read-only hint pass through, and so does its result.

The Mac finds them in two places, without starting any of them:

- Apps that ship a server: Xcode (`xcrun mcpbridge`) and Figma (its local server on
  `http://127.0.0.1:3845/mcp`, once it's turned on in Figma).
- Servers listed in other assistants' settings on the Mac: Claude, Claude Code, Cursor
  and VS Code. Only commands and loopback addresses count; remote servers aren't on the
  Mac.

Each server is its own permission group, `mcp:<id>`, and stays **off** until the owner
picks Ask or Allow under App servers on the Permissions page. The approval mode never
turns one on, though Ask for Everything (and Ask Before Changes, for servers that change
things) makes an allowed one ask first. A server whose command or settings change after
it's turned on is off again until the owner turns it back on. Familiar lists a server's
tools only once it's on, and starts a command server only when it's needed. An assistant calling a tool of a server that's off gets
`DENIED`.

## Errors

Every failure is `{ "error_code", "message", ... }`:

| Code | Meaning |
| --- | --- |
| `APPROVAL_PENDING` | The owner must approve in Familiar. Tell them; repeat the identical call after (it won't ask twice). Has `approval_id`. |
| `APPROVAL_DENIED`, `DENIED`, `PAUSED` | Stop and tell the person. |
| `MAC_STARTING` | The Mac is booting or waking. Wait `retry_after_ms`, repeat. |
| `MAC_UNAVAILABLE` | Stopped, deleted, or the agent dropped. The message says which. |
| `MAC_LOCKED` | The Mac is encrypted and the server restarted since its owner last signed in. Ask them to sign in to Familiar (`sign_in_url`), then repeat. |
| `BUSY` | Too many calls at once on this Mac. |
| `INVALID_ARGS`, `NOT_FOUND`, `STALE_REF` | Fix the call (read the UI again for `STALE_REF`). |
| `PERMISSION_DENIED` | A macOS privacy setting is missing in the Mac; `hint` says which. |
| `TIMEOUT`, `OUTCOME_UNKNOWN` | It may have happened. Check before repeating. |
| `UNSUPPORTED` | The agent in the Mac is too old for this tool. The owner updates it in Familiar. |
