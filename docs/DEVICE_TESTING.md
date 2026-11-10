# Physical-device testing

Run the same ClipClub build on at least two physical machines. The release targets are Apple Silicon macOS and Linux x86-64; include both when available, plus an older/slower machine or a second display scale. Use disposable test clips with visible motion and audible sound: one MP4/H.264 and one WebM/VP9, plus portrait and landscape clips within the upload limits.

Record the device name, OS version, ClipClub version, network arrangement, and which machine is seeding. Use Diagnostics on each device, mark the hands-on checklist, and save the report before closing the app. Keep a separate report for each network arrangement and direction.

## What the submenu checks

| Option | Automated evidence | Hands-on follow-up |
| --- | --- | --- |
| All checks | All supported diagnostic suites | Work through the device checklist |
| Device and storage | Build/platform details, available profile storage, temporary write/read/delete checks | Resize the window; relaunch and verify the library |
| Discovery and peers | Current discovery/mesh observations and bounded TCP listener probes | Transfer a clip in each direction; repeat after reconnecting |
| Video and playback | Validator availability and current library/download observations; webview capability observations | Play both formats with sound, seek, and resize |
| Wallet and chain | Local wallet/session observations and configured read-only chain checks | Lock/unlock, backup/restore, and cancel an approval |
| Device checklist | Your recorded observations | A recorded pass means you performed that step |

Reported codec support does not establish that a particular file decodes correctly. A TCP connection does not establish a ClipClub handshake, a completed transfer, or playback. Discovery observations describe the running app; they are not an independent multicast packet capture. Focused device, media, and wallet suites do not run TCP peer probes. Wallet checks do not sign or submit transactions.

## Repeatable device and network matrix

| Run | Arrangement | Repeat with |
| --- | --- | --- |
| 1 | Both machines on the same normal Wi-Fi/LAN | A seeds to B, then B seeds to A |
| 2 | A different access point or phone hotspot | Both transfer directions |
| 3 | Devices on separate internet connections | Both directions; record whether a connection actually forms |
| 4 | Wi-Fi disconnect/reconnect, then sleep/wake | The arrangement that worked in run 1 |

For each run, keep both apps open for at least 15 seconds before checking discovery. If discovery is empty, enter the other device's reachable IPv4 address and its displayed peer/video ports in Diagnostics. A private LAN address from another internet connection will not ordinarily be routable. Record guest-network isolation, VPN use, or router restrictions alongside the result. A failure on a restricted network is evidence about that setup; compare it with run 1 before attributing it to the application.

## Hands-on checks

1. **Launch and layout.** Launch both apps, check menu navigation and Escape/back behavior, then resize to the smallest comfortable window and a large window. Check text, buttons, dialogs, scrolling, and video controls at each device's display scale. Use keyboard navigation as well as the pointer.
2. **Discovery and transfer.** Upload a new test clip on A. Verify that B discovers A, receives metadata, finishes downloading, and plays the matching clip. Reverse the roles using a different clip. Save diagnostics from both ends when a transfer stalls; metadata alone does not count as a transfer pass.
3. **Playback.** Play MP4 and WebM clips, portrait and landscape, on each device. Confirm motion, sound, mute/volume, pause/resume, seeking, looping in Rate, and controls after resizing. Record a format-specific failure separately from a network failure. Roll playback can be tested when a settled result is available; it should play once.
4. **Session behavior.** Use a zero/free skip and verify that the next eligible clip appears. Leave Rate and return; confirm that the interface recovers and downloads continue. Re-upload the exact same file and verify that it is treated as a duplicate. Remove a test upload and confirm it leaves the permanent library.
5. **Recovery.** Disconnect Wi-Fi, wait, reconnect, and verify that peers and transfers recover. Repeat after sleeping/waking one machine and after quitting/relaunching it. Confirm that permanent uploads survive relaunch and that an interrupted download can make progress again. Rerun diagnostics and record how long recovery took.
6. **Storage.** Compare the storage checks on both devices. Verify upload, completed playback, and restart using the intended profile location. If testing a full or read-only volume, use a disposable profile and record the error shown; confirm that the app remains usable after space or access is restored.
7. **Wallet.** With a disposable wallet, lock/unlock it and restart to verify the startup lock. Export an encrypted backup, restore it in a separate disposable profile, and compare the public address. Keep secrets and passwords out of diagnostic notes. If a transaction review is available, verify the network, amount, and fee fields, then cancel it and confirm that nothing is submitted. No funded wallet or real payment is needed for the baseline device pass; record payment/roll settlement as untested unless separately exercised.

Use **Pass**, **Fail**, or **Skip** for every checklist item and describe a failure with the exact step, expected result, observed result, and the other device's role. Save reports from both ends of a failed transfer. Reports contain network addresses and peer identities, so review them before sharing.
