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
es.bind("Super+Shift+Space", "toggle_window_mode")
es.bind("Super+M", "toggle_minimize")

-- Quit eswm
es.bind("Super+Alt+Backspace", "quit")

-- Window decorations (border size/gaps/colors, corner radius, titlebar)
es.dsp.set_border_size(2)
es.dsp.set_outer_gap(10)
es.dsp.set_inner_gap(10)
es.dsp.set_border_color_focused(0, 200, 0, 255)   -- green
es.dsp.set_border_color_unfocused(80, 80, 80, 255) -- gray
es.dsp.set_titlebar_color_focused(40, 40, 60, 255)   -- dark slate when focused
es.dsp.set_titlebar_color_unfocused(30, 30, 30, 255) -- dark gray when unfocused
es.dsp.set_corner_radius(0)                        -- stored; renderer clipping TBD
es.dsp.set_titlebar_height(0)                      -- 0 = no titlebar; eswm-drawn titlebar when > 0
es.dsp.set_layout_mode("tiling")                   -- "tiling" or "floating"; default mode for new windows
