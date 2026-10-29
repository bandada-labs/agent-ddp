# Decision log

The full, dated history of the maintainer's decisions, with their words: the original (often Spanish) and a translation. Nobody reads it whole; search it by date or topic. The rules in force are in [DECISIONS.md](DECISIONS.md). New entries go at the end.

## 2026-10-23

### README for agents
"sabes estaba pensando que en el readme debería decir lo que ya está del ddp y abajo algo como 'pásale esto a tu agent' y ya, right away nadie está usando eso a mano"
(The README should say what ddp already does and below it something like "give this to your agent"; nobody uses it by hand.) Later: drop the line "Nobody runs this by hand."

### Install like agent-browser
"yo quiero que sea igual de instalable que agent-browser"
(It should be as easy to install as agent-browser.) Led to the npm package with one native binary per platform.

### Rust CLI
Asked to choose between Python and Rust before publishing: "Rust primero" (Rust first). The Python CLI was rewritten in Rust with the same output byte for byte (ADR 0002).

### A flight recorder
"espera espera, podríamos hacer algo como qué pasó hace 15 minutos? o ya es mucho? me imagino al agent usando eso!"
(Wait, could we do something like "what happened 15 minutes ago"? Or is that too much? I can picture the agent using it.) Built on Android first (ADR 0005).

### Where it lives
"que sea bandada-labs pero que yo salga como developer y que se instale solo con npm i agent-ddp"
(It should be bandada-labs, but with me as the developer, and it should install with just `npm i agent-ddp`.)
"yo quería que esté dentro de bandada-labs y disponible para instalar sin bandada-labs"
(I wanted it inside bandada-labs and installable without the bandada-labs scope.) Done with an unscoped package and a read-write grant to the org's developers team (ADR 0003).

### Releases
"creo que es mejor usar GitHub Actions, creo que tengo unos gratis y los publicamos cada que yo diga, no tiene que ser todo el rato"
(Better to use GitHub Actions; I have free minutes; we publish whenever I say, not all the time.) npm 2FA ruled out a token, so npm uses Trusted Publishing (ADR 0003).

### Only agent-ddp in the public repo
"solo debe hablar exclusivamente de agent-ddp, nada de ningún proyecto aparte de ese"
(It must talk only about agent-ddp, nothing about any other project.)

### npm page
"quiero que uses como referencia estos dos paquetes para armar la página [agent-browser, agent-device]; la mía está muy malita"
(Use these two packages as the reference for the page; mine is very poor.) Then: a header with badges "bien lindo", with the project's colors and a styled look.

### Open source level
"también necesito que agent-ddp esté a la altura en formalismo, estructura y arquitectura... para colaboradores teniendo en cuenta que es open source"
(agent-ddp has to be up to standard in formality, structure and architecture, for contributors, given it is open source.) Led to ARCHITECTURE.md, CONTRIBUTING.md, SECURITY.md, the protocol doc and this docs folder.

### Protocol version, weak points, iOS history, project memory
"arregla lo del protocol, luego los puntos débiles y luego agrega iOS porque creo que falta para lo de la caja negra, no? y porfa continúa hablándome en inglés y copia la estructura de memoria y decisions"
(Fix the protocol version, then the weak points, then add iOS, which I think is missing for the black box. Keep talking to me in English, and copy the memory and decisions structure.)

### Release 0.1.5 and security reports
Asked to push and release 0.1.5: "yes". Asked to turn on private vulnerability reporting and give a contact email for a code of conduct: "yes but no notifications". Private reporting is on; no contact email, so no code of conduct for now.

### Name in prose
"recuerda que cuando se hable de agent-ddp debería ser Agent-DDP, ¿no?"
(Remember that when we talk about agent-ddp it should be Agent-DDP, right?)

### CDP as the protocol
"no me interesa mucho que sean opciones de ddp, me hago entender? sino que sea casi que una copia exacta de cdp".
(I don't want them as ddp options, I want almost an exact copy of CDP.)

After asking what QuickJS is, why not plain Java and why not Rust: "bueno aprobado anota todo, y dale arranca". (Approved, write it all down and start.) QuickJS on Android, JavaScriptCore on iOS, the CDP server in Swift and Kotlin, no Rust core for now. ADR 0007.

### Reference client for CDP
After a long try to get Chrome DevTools to show the app: "sí, deja ya eso aparte, es bobada". (Yes, leave that aside, it's silly.) agent-browser is the client the CDP work is checked against. Chrome DevTools is not a goal.

### Phases in order, and what the CDP work is for
"sigamos en orden con las fases, y recuerda que la idea también es ahorrar tokens, optimizar memoria y todo, por eso usamos Rust, adicionalmente debemos ser capaces de ofrecer mejores cosas que agent-device".
(Let's keep the phases in order. Remember the idea is also to save tokens and memory, that's why we use Rust. And we must offer better things than agent-device.)

### CSS through the JavaScript DOM
After hearing that agent-browser reads styles with `getComputedStyle` in JavaScript and never calls the CSS domain: "ah no, sabiendo eso estamos super bien alineados entonces, el CSS se cubre pero por otros medios, buenísimo". (Knowing that, we are well aligned. CSS is covered by other means.) Phase 2 closes without the CSS domain. Styles come in phase 4, from `getComputedStyle` in the JavaScript DOM, with CSS names for the values the library captures.

### Touches on iOS
Asked how `Input` should touch the app on iOS, where no public API lets an app touch itself. Picked "Synthesized touch": private UIKit and IOKit calls, the way KIF and EarlGrey do it, Debug only. Real touches, so gesture recognizers, SwiftUI, drags and scrolling work. A new iOS version can break it and then we fix it. Built as an IOHIDEvent handed to UIKit (Lyft's Hammer approach): KIF's UITouch path reached the view but not SwiftUI's gesture recognizers.

### Native core, browser names only outside
After seeing Storage built as "localStorage" and the plan for a JavaScript DOM with `getComputedStyle`: "osea la idea es usar como cosas parecidas mismos api nombres y cositas así pero en el core debe ser android y ios ok? me preocupa qué tanto de agent-ddp está así, porque está mal". (The idea is to use similar things, the same API names and so on, but in the core it must be Android and iOS. I'm worried about how much of Agent-DDP is like that, because it's wrong.)

Then: "yo no quiero compatibilidad 1 a 1, solo quiero que se use agent-browser como referencia, anota eso, no quiero tener que repetirlo constantemente, es referencia no literal, es para ayudarnos como guía". (I don't want 1:1 compatibility. I only want agent-browser used as a reference. Write it down, I don't want to keep repeating it. It's a reference, not literal, it's there to help us as a guide.)

This replaces "CSS through the JavaScript DOM": no `document` and no `getComputedStyle` in phase 4. ADR 0007 is amended.

### A native protocol instead of CDP
Asked: "mi preocupación es porque estamos convirtiendo algo en que no es propio de Android e iOS, entonces estamos convirtiendo los conceptos y eso puede complejizar las cosas, ¿no? Más bien definamos cuál es el camino correcto. ¿Cuál sería la mejor forma de manejar estos conceptos, como CSS o de su forma nativa?" (My worry is that we are turning something into what is not native to Android and iOS, converting the concepts, and that can make things complex. Let's define the right path: CSS-like or their own native form?)

Proposed: native concepts end to end, agent-browser as a UX guide for the CLI, keep the WebSocket transport and the native cores, drop the fake DOM and the JavaScript DOM plan, replace ADR 0007 with ADR 0008. Answer: "yes go ahead and continue".

### Phases by number, device settings stay in the CLI
Asked "fase 3 no?" after I named the phase "acting". The phases are now numbered 1 to 7 in STATUS.md. Then "bueno continua, recuerda ir actualizando los archivos .md y todo el historial para que no pierdas el hilo". (Go on, and keep the .md files and the history updated so you don't lose the thread.)

The last open part of phase 3 was device settings. `ddp set` already changes them on the device with `simctl` and `adb`, which is the native path: the app gets the same change a user makes. Nothing was added to the library, and phase 3 is closed.

### Scripts are app commands, not JavaScript
Asked why I had said JavaScript before. JavaScript came from the CDP plan (`Runtime.evaluate`); under ADR 0008 it has no reason left, Android has no engine without a new dependency, and it cannot reach the app's Swift or Kotlin without a bridge. Proposed app commands: named closures the app adds in Debug. Asked whether they are Kotlin and Swift and whether they let the app send a request: yes, through the app's own API client, plus a built-in `net.send` with the app's client. Answer: "buenísimo hagamos eso entonces". (Great, let's do that.)

### Phase 6: the CLI on the control endpoint
Proposed the order: a small WebSocket client in the CLI with the standard library (the server is ours and local, so no crate), the existing commands first with the same output, then new commands with agent-browser as a guide, then phase 7. Answer: "continue with phase 6".

`ddp tap` on iOS was the accessibility action of the v1 endpoint. It is now `input.touch`, a real finger, as on Android.

### Phase 7: v1 goes
Answer: "continue with phase 7". The v1 JSON lines endpoint is removed from both libraries. The control endpoint had its own version (2) and the headers had theirs (1); now there is one, 2, as ADR 0008 planned. An old CLI reading a new history file stops with `protocol_too_new`, which is right, since it cannot talk to the new library either.

### A button that shows but cannot be tapped
A Reddit user wrote: "I'd test a button that's visible but can't actually be tapped. Correct bounds alone wouldn't convince me the agent can use it." Asked how CDP solves it: CDP has the pieces (`DOM.getNodeForLocation`, the AX tree's `disabled`) and Playwright checks before each click (visible, stable, enabled, receives events). Asked what ddp can offer and whether it is an `eval`: not an `eval`, a check in `tap` plus `check tappable`, and from inside the app the native reason (disabled, covered, a parent that takes no touches, outside the parent's bounds). The maintainer said they do not know devtools; I said the approach is the one Playwright, XCUITest (`isHittable`) and Espresso use, and proposed a test first because SwiftUI and Compose are not UIViews. Answer: "yes, start the test and based on that implement it".

The test (2026-10-24): four trap buttons in each sample app. iOS: the accessibility hit test sees through clear views and `allowsHitTesting(false)`, so it is not used; the hosting view's `_hitTestWithContext:` returns the SwiftUI gesture that would get the touch, and caught all four. Android: Compose's `LayoutNode.hitTest` by reflection caught disabled, covered and outside; a parent that consumes touches in the Initial pass passes any hit test, and `tap` reports `nothing changed` for it.

### Parents that take the touch on its way
Asked whether the browser finds these through the event tree. It does not: agent-browser and Playwright fail a click "before dispatch" with `covered by <...>`, from a hit test at the click point; nothing checks the capture and bubble path ahead of time. Listed the cases a hit test misses because a parent takes the touch on its way down (iOS: a parent `UITapGestureRecognizer` over a table row, a SwiftUI `highPriorityGesture`; Android: `onInterceptTouchEvent`, a Compose `PointerEventPass.Initial` consumer) and asked whether to cover them in 0.2.0 or 0.2.1. Answer: "es que esa es la vaina, es la primera vez que hago un devtool, entonces no se que debemos cubrir y que no, en mi ignorancia digo que todos esos casos se deben cubrir porque la diea es que muchos agentes los usen y entre mas casos cubiertos mas errores se reportan". (The thing is, it's my first devtool, so I don't know what we should cover; I'd say all those cases, since many agents will use it and the more cases covered, the more errors get reported.)

Done in 0.2.0, with one rule: no false alarms. A case that cannot be decided without a real touch (a recognizer whose delegate decides per touch, a parent that takes only the release) is not reported; `tap` still shows it after the touch. iOS reads the gesture containers UIKit and SwiftUI chain from the hit (`_parentGestureRecognizerContainer`, private), and asks the hit view `gestureRecognizerShouldBegin`, which is how a UIButton wins over a parent tap. Android asks each parent `onInterceptTouchEvent`, and each Compose parent node the Initial pass, with a press and then a cancel. Every case has a trap in the sample apps, confirmed with a `--force` tap that changes nothing. The same work found a false alarm: a SwiftUI button in a hosting view nested in UIKit read as untouchable, because the gesture hit test takes window coordinates.

### Zero false positives is the hard rule
While the parent cases were being built, the maintainer wrote: "mete lo de zero false positive como hard rule es mas importante que cualquier otra cosa". (Make zero false positives a hard rule, it's more important than anything else.) It is now the first rule in ARCHITECTURE.md and AGENTS.md: nothing ddp reports as a problem may be wrong, an undecidable case is not reported, and every check has a case in the sample apps where it must stay quiet.

### The engine is the source of truth
Asked whether `check tappable` now covers every case. Answered that it does not, and that part of it was our own model: on iOS no API says which gesture wins before a real touch, so the parent `UITapGestureRecognizer` and `highPriorityGesture` cases were decided by our rules over the recognizers' state. The maintainer: "oye pero me preocupa yo queria que ddp solo usara el motor de cada plataforma, si no terminariamos haciendo una especie de reduced engine [...] esperaba que usaras las cosas nativas de cada uno de esa forma aseguramos que todo es fidedigno y apuntando al engine absoluta confianza". (I'm worried, I wanted ddp to use only each platform's engine, otherwise we end up with a kind of reduced engine; I expected the native pieces, so everything is faithful and points at the engine with full trust.) Proposed removing the two iOS cases. Answer: "quiero que metas eso tambien como hard rule necesitamos asegurar confiabilidad y certeza contra el engine que se esta usando es nuestra fuente de confianza". (Make that a hard rule too, we need reliability and certainty against the engine being used, it is our source of trust.)

Done: the second hard rule in ARCHITECTURE.md and AGENTS.md. The two iOS `intercepted` cases are removed; `tap` still reports `nothing changed` for them. The Android ones stay, since there the app's own `onInterceptTouchEvent` and Compose pointer nodes answer. The other places that still model the engine are listed in STATUS.md.

### Hooks are fragile; no reduced engine; every hook measured
Listed the seven places that still modeled the engine and proposed fixing the small ones first. Answer: "fix them first and everything should be working fine". While that was being done the maintainer asked: "hay una cosa que hay que tener en cuentas, puede que ciertas cosas no funcionen bien que cdp si ofrece porque el moto es capaz, pero que pasa si lo que ahcemos son puros workarounds o cosas inestables? duda, deberiamos usar un reduced engine? duda" (Some things CDP offers work because the engine exposes them; what if ours are only workarounds or unstable? Should we use a reduced engine?) Answered no: a reduced engine is wrong without anyone seeing it, a broken hook fails closed and can be shown. Then: "quiero que tambien metas una hard rule para medir la inestabilidad de cada hook y funcionabilidad" (Make a hard rule too to measure each hook's instability and whether it works.)

Done: the third hard rule in ARCHITECTURE.md and AGENTS.md; `Hooks` in both libraries, `app.hooks`, hooks in `ddp doctor`, docs/hooks.md. The seven places:
1. Android Views, hit test: the walk uses ViewGroup's own methods; app code that decides a touch is unknown. Android 15 blocks two of the methods; their documented rule answers and is counted as a fallback.
2. iOS SwiftUI, whose gesture: SwiftUI's responders, read from the accessibility node's geometry. Found on the way: SwiftUI hit-tests a touch with UIKit's 20 pt radius, so a radius 0 test called reachable points blocked; now the radius is UIKit's own, watched from its call.
3. overlap and contrast, and 4. corners: the app's own pixels (`screen.pixels`), between two frames. A device screenshot was tried first and dropped: a system alert over the app showed in it. iOS SwiftUI text is alpha-only and colored by the render server, so iOS samples the screen at each text's own ink and clear points.
5. touch size: the app's hit test from the center out; SwiftUI's radius and Compose's 48 dp widening count, so a lone small control is fine and a crowded one is not.
6. jank: the libraries count missed refreshes from the platform's frame times, their own capture left out.
7. `get --curve`: says which named curve the values match, not which one the app ran.

### Hook failures reported; a crash-safe responders read
Asked about the hooks' instability. Answered that the counts catch a hook that breaks but not one that answers wrong, that every hook was checked on one version only, and that `swiftui.responders` read word 2 of a Swift metadata record without a check, so a layout change could crash the app. Proposed a known-answer probe at start, a guard on that read and a version matrix. Answer: "yes, do it but the first one would take time and resources i'm not sure about it also i need them to report it and let the agent know it and available for the user".

Done: the guard (the attribute must still be a one-argument `WeakAttribute` struct and word 2 must name that argument, else the hook fails with why); every check, `check tappable` and `explore` print the failed hooks, and the take keeps them in `inspect.hooks`. The probe is not built, waiting on the maintainer.

## 2026-10-27

### 0.2.1 published before the two-phone test
Mimbre's asks were cut to a patch: `ddp compare` and a note on headless simulators. The other asks wait for a later release. Asked whether to wait for a free device slot to run `compare <element> --on ios,android` on a real simulator and emulator. Answer: "ve haciendolo tu lo de publicar el nuevo release". Published with the fake-app tests and a live screen compare on the iOS simulator. The two-phone run happens when a slot is free, and a bug there ships as 0.2.2.

## 2026-10-29

### Three answers for 0.3.0
Triage of the 0.3.0 tickets raised three questions. Answers: "1. ok 2. ok 3. its ok".
1. History on Android ran the layout walk every frame, and with a looping animation an idle debug app used 30-110% CPU and hit an input ANR. Asked whether skipping frames where nothing changed is fine, keeping the 15 minutes of ADR 0005, instead of pausing History when no recording is open. Yes.
2. `compare` always exits 0 since 0.2.1. Asked whether an opt-in `--max <pct>` that exits non-zero is fine, since the caller sets the bar and ddp reports no problem of its own. Yes.
3. A structure compare across phones was asked to work without the library. Asked whether the accessibility tree is enough for that. Yes.
