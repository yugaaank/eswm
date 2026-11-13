-- eswm Lua configuration (~/.config/eswm/config.lua)
-- Loaded on startup; defines keybinds and autostart via the `es` Lua API.

-- Autostart (spawned on session start)
es.dsp.exec_cmd("awww-daemon")
es.dsp.exec_cmd("fuzzel")
es.dsp.exec_cmd("awww img /home/user/Pictures/Wallpapers/wall.jpg")

-- Keybinds: es.bind("modifiers+key", "action", [arg])
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")

-- Workspace switching: Super+1..9
for i = 1, 9 do
    es.bind("Super+" .. i, "switch_workspace", i)
    es.bind("Super+Shift+" .. i, "move_window_to_workspace", i)
end

-- Focus cycling
es.bind("Super+Left", "cycle_focus_prev")
es.bind("Super+Right", "cycle_focus_next")

-- Window swapping
es.bind("Super+Ctrl+Left", "swap_focused_prev")
es.bind("Super+Ctrl+Right", "swap_focused_next")

-- Workspace switching relative
es.bind("Super+Alt+Left", "focus_workspace_prev")
es.bind("Super+Alt+Right", "focus_workspace_next")

-- Window mode toggles
es.bind("Super+Space", "toggle_window_mode")
es.bind("Super+Shift+Space", "toggle_layout_mode")
es.bind("Super+f", "toggle_fullscreen")
es.bind("Super+m", "toggle_maximized")

-- Launcher
es.bind("Super+Alt+Space", "spawn_fuzzel")

-- Quit
es.bind("Ctrl+Alt+Backspace", "quit")

-- VT switching
for i = 1, 12 do
    es.bind("Ctrl+Alt+vt" .. i, "change_vt", i)
end

-- Run arbitrary commands on keypress: es.bind_cmd("modifiers+key", "command")
es.bind_cmd("Super+r", "rofi -show drun")
es.bind_cmd("Super+e", "emacsclient -c")
