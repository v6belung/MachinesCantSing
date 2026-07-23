; Clears the classification cache before installing over an existing copy, so every artist
; gets re-evaluated under whatever detection logic the new version ships with, instead of
; keeping verdicts reached under older, less accurate heuristics. Harmless no-op on a fresh
; install -- Delete silently does nothing when the file isn't there.
!macro NSIS_HOOK_PREINSTALL
  Delete "$APPDATA\dev.v6belung.now-playing-flagger\now-playing-flagger.sqlite3"
  Delete "$APPDATA\dev.v6belung.now-playing-flagger\now-playing-flagger.sqlite3-shm"
  Delete "$APPDATA\dev.v6belung.now-playing-flagger\now-playing-flagger.sqlite3-wal"
!macroend
