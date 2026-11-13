-- eswm Lua configuration (~/.config/eswm/config.lua)
-- Loaded on startup after config.json; defines keybinds via es.bind().

-- Spawn a terminal on Super+Return
es.bind("Super+Return", "spawn_terminal")

-- Close focused window on Super+Q
es.bind("Super+q", "close_window")

-- Workspace switching: Super+1..9
for i = 1, 9 do
    es.bind("Super+" .. i, "switch_workspace")
    es.bind("Super+Shift+" .. i, "move_window_to_workspace")
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
    es.bind("Ctrl+Alt+vt" .. i, "change_vt")
end

-- Autostart via Lua (alternative to config.json autostart)
es.dsp.exec_cmd("awww-daemon")
es.dsp.exec_cmd("fuzzel")
