# VHS demo: keito timer workflow
# https://github.com/charmbracelet/vhs

Output recordings/time-start-stop.gif

Set Shell zsh
Set FontSize 28
Set FontFamily "Menlo"
Set Width 2400
Set Height 1350
Set Theme "Catppuccin Mocha"
Set TypingSpeed 50ms
Set Framerate 24

Type "# Start a timer for a project"
Enter
Sleep 1s

Type "keito time start --project 'Acme Website' --task dev"
Enter
Sleep 2s

Type "# Check what's running"
Enter
Sleep 1s

Type "keito time running"
Enter
Sleep 2s

Type "# Stop the timer"
Enter
Sleep 1s

Type "keito time stop"
Enter
Sleep 2s
