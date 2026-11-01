from pathlib import Path
path = Path('README.md')
text = path.read_text()
text = text.replace('- **Roll:** coming soon. No gambling or point deduction is implemented.', '- **Roll:** wager points for a random integer multiplier from 0 through 9. A 0 loses the wager and plays no clip. Positive results pay `wager × multiplier` in total and play one random clip from that rating tier, without looping. Previously watched clips and clips in your own library are eligible.')
text = text.replace('Every identity starts with **100 points**. A rating costs the rater its score, from 0 through 9.', '''Every identity starts with **100 points**. A rating costs the rater its score, from 0 through 9.

Roll accepts positive wagers up to your balance, in millionths of a point. The wager is deducted once and the total payout is `wager × k`: rolling 1 returns the wager, while rolling 9 returns nine times the wager. Each multiplier has probability `P(k) = 2^(-k) × (512 / 1023)` for `k = 0…9`. The exact outcome weights are 512, 256, 128, 64, 32, 16, 8, 4, 2, and 1 out of 1023.

For a positive roll, the clip tier is its average positive rating rounded to the nearest integer. Unrated clips are excluded. Selection includes previously watched clips and your own clips; playing a roll does not submit a rating or change watched history. If no clip in that tier is available, the result and payout still stand and the interface explains why no clip can play. Rolls share the signed receipt history with ratings, so point changes persist across restarts and can be validated by peers.''')
text = text.replace('Signed receipts identify their author but do not prove actual seeding or honest population reports.', 'Signed receipts identify their author but do not prove actual seeding or honest population reports. Roll draws are generated locally and recorded in signed receipts; peers can verify the recorded multiplier and payout, but cannot prove the draw was random or prevent a modified client from choosing outcomes.')
text = text.replace('Tests cover rating math, signatures and persistence, file limits and malformed media, deterministic torrents and unsafe metadata, and peer identity/message validation.', 'Tests cover rating and roll math, signatures and persistence, roll playback behavior, file limits and malformed media, deterministic torrents and unsafe metadata, and peer identity/message validation.')
path.write_text(text)
path = Path('scripts/check.sh')
text = path.read_text().replace('node --test scripts/library.test.cjs', 'node --test scripts/library.test.cjs scripts/roll.test.cjs')
path.write_text(text)
