# GTAxSkate's additions to SK8V

**To play:** download [GTAxSkate-Setup.zip](https://github.com/Manatros/GTAxSkate/releases/latest/download/GTAxSkate-Setup.zip), unzip, double-click `Install GTAxSkate.cmd`. Help: [GTAxSkate](https://github.com/Manatros/GTAxSkate).

This fork of [SK8V](https://github.com/becaring/SK8V) (by becaring) is what
[GTAxSkate](https://github.com/Manatros/GTAxSkate) plays: SK8V as it is, plus a
set of additions, each in its own branch so SK8V can take any of them as a pull
request. The branch `skatemods` is SK8V's newest release with every addition
merged; the GTAxSkate installer installs a build of it.

## Why it is built this way

GTAxSkate started as its own Skate 3 mod for GTA V, with its own Skate core,
ground probes, park collision, board sounds and a lot of board customization.
SK8V does the core better: Skate's own audio engine, the pose hook, live
collision, Hall of Meat, skitching, guns on the board. So the decision (Marc,
10-10) was:

1. **SK8V is the base.** Skating, animation, collision, audio and the HUD stay
   SK8V's. GTAxSkate does not keep a second core.
2. **GTAxSkate fills the gaps,** and gives each one back to SK8V in SK8V's own
   code and style, so becaring can take it as it is and grow it.
3. **Every addition is independent:** one branch per feature, cut from SK8V's
   `main`, its own INI keys and menu entries, README and INI template lines.
   Runtime functions are optional exports (looked up with `GetProcAddress`), so
   an older runtime works with a newer host and the other way round.
4. **SK8V's behaviour stays SK8V's by default.** Options that change how SK8V
   plays (GTA's camera, knocked people surviving) are off in SK8V's template;
   the GTAxSkate installer turns on what Marc prefers.
5. **The best working solution wins.** Where SK8V already does something well,
   GTAxSkate's version was dropped; where GTAxSkate's worked better (the vehicle
   shapes, the board shop), it was ported into SK8V's structure.

## The branches

| Branch | What it adds | How it works |
|---|---|---|
| `contrib/keyboard-mouse` | Keyboard and mouse as a controller; the keys on screen; every key rebindable in the menu; F gets on and off; F10 opens the menu | `rust/skate-input` turns held keys into a controller frame (the left stick ramps like a thumbstick, the trick stick is typed with the arrow keys, Space is the pop: held = crouch, let go = flick). The host uses it alongside a controller: whichever was used last plays, so an idle or virtual pad (Steam Input, DS4Windows) never shuts the keyboard out. Keys in `SkateVKeys.txt`; the menu page "Keyboard keys" rebinds them (`sv_keyboard_bind`). |
| `contrib/board-shop` | Skate 3's board shop in GTA's street clothing stores; the bought board ridden; boards on show; wear | Every deck, truck, wheel, griptape and paint of Skate 3 is built (from the player's own disc) as a drawable with the native board's seven-bone skeleton (`tools/prepare-board-parts.py`), so trucks turn and wheels spin like SK8V's board. The host loads only the chosen parts from loose files (no DLC, no archetype limit) and poses them with SK8V's board pose. The runtime keeps the catalog, prices, each character's garage and the store's show (`shop.rs`, `board_shop.rs`). G (or E while looking at the boards on show) opens it; cash is the character's own. |
| `contrib/gta-camera` | A third person camera while skating (near, middle, far) that keeps out of the skater, the view kept between walking and skating, the skater's eyes, R a cinematic camera and a skate filmer that look filmed, a Camera menu page | Near, middle and far follow the board, not the body, so flips and bails never swing the camera into the skater; it swings in behind the direction of travel and the mouse turns it. The view last used carries over between walking and skating (`CameraRemember=`), `CameraView=` is the standard one; first person is GTA's, the skater's eyes sit at Skate's head. The cinematic camera gets 2.39:1 bars, a film grade and shallow focus; the filmer 4:3 and a handheld shake. The cinematic camera (`action_cam.rs`, tested) cuts between chase, roadside, side, overhead, wheel, telephoto and front shots like GTA's in a car. |
| `contrib/trainer-tuning` | Pop, grind pop, push, braking, steering, wobble, grip, turn power, manual drag, hold fakie, hippie jump height; stamina | Skate's `TrainerTuning` multipliers were read but never set: overlay patch 0049 sets them, patch 0050 adds the hippie jump's height (1.6 clears a car). GTA's stamina skill scales push and pop (85 % with none); skating at speed trains it. |
| `contrib/vehicle-shapes` | Cars you can ride and hippie jump that SK8V only knows as boxes | SK8V's extracted bounds miss many vehicles (most DLC cars). Such a model is measured once with up to 500 probes down its own up axis; the heights make a roof, hood and trunk, walled down to 0.2 m above the ground (`vehicle_shapes.rs`, tested), used instead of the box. |
| `contrib/map-helpers` | GPS route while skating; GTA's skate parks on the map | GTA drops the route when the player is neither on foot nor in a vehicle; it is drawn as a vehicle's. The parks were found in GTA's own map data. |
| `contrib/skate-radio` | Skating tunes the radio to a chosen station (Self Radio: Skate 3's soundtrack) | Once per skate session, unless the radio is already on; says once if Self Radio is empty. |
| `contrib/injuries` | A limp after hard bails | The worst bone knock of a bail (SK8V's bail grunt measure) from 9 m/s on gives GTA's `move_m@injured` walk on foot for 30 s to 2 min. |
| `contrib/board-shop` (board in hand) | see above: the board picked in the wheel stays in hand in GTA, swung with GTA's melee, blows wear it | `fork-extras` holds the CarryBoard switch; `boardparts::Hand` shows the HD parts on SK8V's hidden hammer. |
| `contrib/gentle-peds` | Option: people the board knocks down survive | Invincible from the knock until they are up again. |


`fork-extras` holds two small fork-only changes (a runtime handle export, and
Y with the board in hand getting on). `skatemods` is `fork-extras` plus a merge
of every branch above, made anew by GTAxSkate's `scripts/sk8v-sync.ps1` on each
SK8V release (it resolves the list conflicts the branches share, checks that
nothing lost a brace, builds and runs the tests).

## Status

Built and unit-tested on SK8V 1.1.8; released as GTAxSkate v0.5.2. Tested in
the game: keyboard and mouse with its layouts, the camera views and cinematic
camera, the board shop with its show, purchases, paints and wear, stamina,
skateable cars and the hippie jump, gentle peds, the limp after hard bails,
F to get on. Open: M while skating does not open GTA's interaction menu.
