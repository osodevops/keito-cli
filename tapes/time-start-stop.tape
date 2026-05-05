# VHS demo: keito timer workflow
# https://github.com/charmbracelet/vhs

Output recordings/time-start-stop.gif

Set Shell zsh
Set FontSize 18
Set FontFamily "Menlo"
Set Width 1800
Set Height 800
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set Framerate 24

# Pre-demo: use the local release binary and clear the terminal (hidden from recording).
Hide
Type "export PATH=./target/release:$PATH"
Enter
Sleep 500ms
Type "setopt interactive_comments"
Enter
Sleep 500ms
Type "PROMPT='> '"
Enter
Sleep 500ms
Type "PROMPT_EOL_MARK=''"
Enter
Sleep 500ms
Type "stty cols 160 rows 40"
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 500ms
Show

Type "# Track billable time from the terminal"
Enter
Sleep 1s

Type "keito --version"
Enter
Sleep 1s

Type "# Pick a project and task"
Enter
Sleep 1s

Type "keito projects show 'Website Redesign'"
Enter
Sleep 3s

Type "keito projects tasks --limit 6"
Enter
Sleep 3s

Type "# Start a timer"
Enter
Sleep 1s

Type "keito time start --project 'Website Redesign' --task Development --notes 'Demo recording'"
Enter
Sleep 4s

Type "# Check the active timer"
Enter
Sleep 1s

Type "keito time running"
Enter
Sleep 3s

Type "# Stop and save it"
Enter
Sleep 1s

Type "keito time stop --notes 'Demo recording complete'"
Enter
Sleep 4s

Type "# Review recent entries"
Enter
Sleep 1s

Type "keito time list --limit 3"
Enter
Sleep 4s
