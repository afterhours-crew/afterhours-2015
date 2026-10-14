# Gameplay checklist

This is the contributor-facing feature checklist for solo AllDrive and garage
gameplay. The order balances player value, dependencies and what is currently
understood. A checked item means its stated scope has been tested in the game;
codecs, stored data or a working menu alone do not complete a gameplay feature.

See [setup and validation limits](LOCAL-LAUNCHER.md) before running the game.
Installed activation requirements remain separate from gameplay coverage.
The current first-item candidate and test-account cash command are documented in
[garage upgrade development](GARAGE-UPGRADES.md); its complete live acceptance
remains unchecked.

## Current baseline

- [x] Build the server and launcher from this repository without private scripts
  or bundled game content.
- [x] Create a separate local account and reach the garage and free roam.
- [x] Show the driving HUD and own-car minimap marker after the exit animation.
- [x] Repeat the garage/world loop, including the entry prompt, confirmation and
  cancellation when driving away.
- [x] Reuse an existing local account after restarting game/server. This does not
  establish that newly earned rewards or purchases are saved correctly.

## Remaining systems, in priority order

1. [ ] **Garage upgrades and customization.** Buy, equip and save changes. Start
   with one performance part, then extend to visual parts, handling/tuning, paint
   and wraps. Inventory, part definitions and transactional storage already exist;
   performance purchases and three handling controls now survive garage return
   and game/server restart. Selected hood, headlight and rim upgrades also pass
   those transitions; rim paint/size survive a tire change and restart. Broader
   visual customization and the complete wrap-edit workflow remain unfinished.
   Saved local wraps now reload after restart; applying one retains body paint
   and the equipped livery across the tested garage/world transitions.
   Plate text, window tint and factory body-paint reset have automated coverage;
   their live menu and persistence checks are pending.
2. [ ] **Free-roam REP and unlocks.** Award driving scores, update levels and make
   newly eligible content available. Current stored-score/level read models do
   not implement earning REP.
3. [ ] **One complete event and its map/world activation.** Model eligibility,
   discovery, start, checkpoints, finish, restart/abandon, results and saved
   cash/REP rewards. A solo time trial is the first candidate.
4. [ ] **Car ownership and garage management.** Buy/sell cars, select the active
   car, maintain the five slots and restore the selection after restart.
5. [ ] **Collectibles and parts inbox.** Activate and complete donut spots,
   vistas and free-part collections; claim rewards once and persist completion.
6. [ ] **Traffic and AI rivals.** Establish client/server responsibility for
   activation, spawning, simulation and replication, then implement missing
   behavior. Traffic-related registrations alone do not establish this feature.
7. [ ] **Police and pursuits.** Support activation, escalation, cooldown/escape,
   busting, fines and associated REP. Simulation ownership still needs research.
8. [ ] **Story and phone objectives.** Calls/messages, mission sequencing,
   objective completion and unlocks, including the currently bypassed prologue.
9. [ ] **Daily challenges.** Advance objectives, complete and claim rewards once,
   rotate a local schedule and retain progress. Current challenges are previews.
10. [ ] **Remaining race and spontaneous modes.** Extend the event foundation to
    sprint/circuit/drift variants and spontaneous challenges, with AI or other
    players where needed.
11. [ ] **Records, awards and sharing.** Populate race records, medals and
    leaderboards; support wrap/snapshot content. Existing read handlers or empty
    local lists are only foundations.
12. [ ] **Multiplayer crews and shared worlds.** Multiple participants,
    synchronized entities, membership changes and reconnects. Keep interfaces
    suitable for this while prioritizing solo offline play.

GPS/waypoints, map travel, the phone's return-to-garage action and detailed
damage/repair behavior need focused capability checks before being classified
as missing server features. Some behavior may already be handled by the client.

## First upgrade acceptance

The initial implementation is intentionally one complete supported purchase,
not a claim that every customization menu works:

- [ ] Obtain the supported transaction contract and installed item/price data.
- [x] Buy and equip a compatible performance part through the garage UI.
- [x] Deduct the supported price exactly once in the same durable transaction.
- [ ] Canceling an edit and insufficient funds leave the account unchanged.
- [ ] Retries cannot duplicate the part or charge; independent accounts stay
  isolated and stale concurrent edits cannot overwrite newer changes.
- [x] The resulting vehicle uses the part in free roam and on garage return.
- [x] Restart both game and server; the part and balance remain correct.
- [ ] Pass portable tests, protocol reference comparisons and a documented live
  run. Record limitations before checking off this acceptance.

Update this checklist as support changes. Keep unchecked work visible rather
than treating a supporting library or synthetic test as complete game support.
