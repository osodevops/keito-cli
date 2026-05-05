# VHS demo: keito timer workflow
# https://github.com/charmbracelet/vhs

Output recordings/time-start-stop.gif

Set Shell zsh
Set FontSize 16
Set FontFamily "Menlo"
Set Width 960
Set Height 540
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
Type "stty cols 112 rows 28"
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

Type "keito projects show 'Website Redesign' --json | jq -r '.name, .code'"
Enter
Sleep 3s

Type "keito projects tasks --json | jq -r '.[0:6][].name'"
Enter
Sleep 3s

Type "# Start a timer"
Enter
Sleep 1s

Type "keito time start --project 'Website Redesign' --task Development --notes Demo"
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

Type "keito time stop --notes Done"
Enter
Sleep 4s

Type "# Confirm no timer is running"
Enter
Sleep 1s

Type "keito time running --json"
Enter
Sleep 4s
