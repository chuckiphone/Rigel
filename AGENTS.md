# Rigel project instructions

## Agent pacing

- Never sleep longer than **30 seconds** in a terminal command.
- For long-running jobs, run them in the background with a log and poll in intervals of 30
  seconds or less. Do not block a command on a multi-minute sleep.
