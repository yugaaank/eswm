-- Default keybind template for eswm.
-- Copy to ~/.config/eswm/config.lua and edit as needed.
-- No keybinds are compiled into the binary; an empty config binds nothing.

-- Workspace switching: Super+1..9
es.bind("Super+1", "switch_workspace", 1)
es.bind("Super+2", "switch_workspace", 2)
es.bind("Super+3", "switch_workspace", 3)
es.bind("Super+4", "switch_workspace", 4)
es.bind("Super+5", "switch_workspace", 5)
es.bind("Super+6", "switch_workspace", 6)
es.bind("Super+7", "switch_workspace", 7)
es.bind("Super+8", "switch_workspace", 8)
es.bind("Super+9", "switch_workspace", 9)

-- Move window between workspaces: Super+Shift+1..9
es.bind("Super+Shift+1", "move_window_to_workspace", 1)
es.bind("Super+Shift+2", "move_window_to_workspace", 2)
es.bind("Super+Shift+3", "move_window_to_workspace", 3)
es.bind("Super+Shift+4", "move_window_to_workspace", 4)
es.bind("Super+Shift+5", "move_window_to_workspace", 5)
es.bind("Super+Shift+6", "move_window_to_workspace", 6)
es.bind("Super+Shift+7", "move_window_to_workspace", 7)
es.bind("Super+Shift+8", "move_window_to_workspace", 8)
es.bind("Super+Shift+9", "move_window_to_workspace", 9)

-- Common controls
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+M", "toggle_minimize")
es.bind("Super+F", "toggle_fullscreen")
es.bind("Super+Space", "toggle_window_mode")
es.bind("Super+Tab", "cycle_focus_next")
es.bind("Super+Shift+Tab", "cycle_focus_prev")
es.bind("Super+E", "focus_workspace_next")
es.bind("Super+W", "focus_workspace_prev")

-- Quit
es.bind("Super+Shift+Q", "quit")
