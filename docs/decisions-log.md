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
