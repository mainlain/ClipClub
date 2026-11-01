# Final points-era compact menu recovery

All artifacts derive from thread `01a1886a-c433-76b1-90ab-8e54312f8528` and its two historical subagents. No shared work/source tree was edited during extraction.

Replay after ratings and paid-Roll recovery:

```sh
python3 /absolute/path/to/extracted/menu/replay.py /absolute/path/to/work
```

The replay changes UI, the historical README menu paragraph, and only main-window numeric dimensions. Fork profile path, default ports, executable identity and window title are preserved. Main-window dimensions already present at 480x620 (minimum360x480) are recorded as already applied.

Chronological feature changes:

1. `00-window.diff`: compact_window subagent `exec-2e88ae12-4c2b-457f-b74c-09072f645396` changes main window from1180x830/min740x620 to480x620/min360x480.
2. `01-html-readme.py`: root `exec-6b5af425-dd3b-451c-979d-29229aa697e7` replaces sidebar shell with header, four-row home menu, back control and footer; hides Rate at startup and updates README navigation documentation.
3. `02-style.css`: full exact root rewrite `exec-6e78eb53-7559-4bf0-9bca-b1a9ed796643` makes each screen fit compact layout.
4. `03-navigation.diff`: home_navigation subagent `exec-5d12d2f2-5037-4b18-9d6c-9190acba3b2a` implements Home startup, serialized session start/stop, Escape/Menu navigation, focus and playback lifecycle.
5. `04-navigation.diff`: subagent `exec-268c8274-19e0-43f4-aeb1-1dde18a546db` restricts handlers to buttons, preventing the new body data-page attribute from hijacking clicks.
6. `05-navigation.diff`: subagent `exec-735c27a9-2474-4a1b-a3fc-d7709e13487b` disables Settings until data arrives and rejects premature submission.

README transform and CSS write are complete, untruncated historical command payloads. All navigation and window diffs are complete, untruncated change records. The exact pre-menu JavaScript readback is complete; replaying all navigation patches therefore reconstructs the final file without guessing. Read-only combined output excerpts elsewhere are truncated, but no feature-writing payload is missing.

Verification replay used the exact pre-menu HTML/JavaScript/readme readbacks in `historical-before/` and produced `verification/`. Result:

- Final HTML matches the direct historical post-menu readback byte for byte.
- CSS is recovered in full from its original write; direct post-menu readback confirms its first9266characters before output truncation.
- JavaScript syntax passes; all13 original library/Roll tests pass.
- Original temporary navigation smoke (now preserved as navigation-smoke.cjs) passes: Home startup makes no session request, and rapid Rate/Home transitions leave session stopped and playback paused.
- No project test files were changed by the original menu feature. The two exact test readbacks are retained under verification/scripts/.

First cryptocurrency boundary excerpts retain Home startup, navigationQueue and button-only selectors. The boundary does not provide a complete pre-crypto UI readback: source excerpts already contain wallet additions, and later Settings code falls outside available excerpts. This is not evidence of a missing points-era revision. The complete menu mutations themselves have no gaps.
