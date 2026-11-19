# Diplomacy with AI players

How a human seat talks and trades with an AI leader on the server. The screens are `diplo`, `leader` and `deal` (`crates/shared/game-host/screens`); the core side is described in the core fork's `headless/portable/ui-seat-queries.md`.

## How the original does it

The original game runs the leader screen on the human's own machine, with that human as the active player.

- **Opening.** The leader list counts the opens per partner (`UI.ChangeStartDiploRepeatCount`) and calls `Player:DoBeginDiploWithHuman`. The AI picks a greeting (`CvDiplomacyAI::GetGreetHumanMessage`: war, peace wanted, friendship, hostility, repeats) and shows it through `gDLL->GameplayDiplomacyAILeaderMessage(ai, DIPLO_UI_STATE_DEFAULT_ROOT, text, animation)`. The greeting changes nothing in the game.
- **Leader messages.** Every line the AI says to the human goes through `GameplayDiplomacyAILeaderMessage(ai, state, text, animation, data)`. The state (`DiploUIStateTypes`) selects what the screen shows: the root, the human-invoked discussion, the trade screen, an AI offer, demand or request, or a plain answer. The exe shows it only on the machine whose active player the line is for.
- **Discussion.** The human's topics (declare friendship, denounce, do not settle near me, stop spying, stop converting, stop digging, a war together) go out as `Game.DoFromUIDiploEvent` → `gDLL->sendFromUIDiploEvent`, a network message. Every machine runs `CvDiplomacyAI::DoFromUIDiploEvent`; the AI's reply is a leader message on the asker's machine only.
- **Trade.** The trade screen holds a scratch deal. `UI.DoProposeDeal` runs `CvDealAI::DoHumanOfferDealToThisAI` on the human's machine: `IsDealWithHumanAcceptable` values it, a gift is accepted too, and an accepted deal goes out as `gDLL->sendNetDealAccepted` with the deal. Every machine then runs `DoAcceptedDeal`, which adds and finalizes the deal (`CvGameDeals::FinalizeDeal` checks every item again) and says the leader's thanks on the proposer's machine. A refusal is a leader line (unacceptable, insulting, or a repeat after several offers). "What would make this work", "what do you want for this" and "what will you give me for this" run `DoEqualizeDealWithHuman` on the screen's deal.
- **Peace and war.** War is `sendChangeWar` after a confirmation. Peace is `FROM_UI_DIPLO_EVENT_HUMAN_NEGOTIATE_PEACE`: the AI either refuses or prepares the trade screen with the peace items and its price (`DoTradeScreenOpened`, a value cached per AI on the asking machine).
- **AI-initiated diplomacy.** `CvDiplomacyRequests` queues the AI's requests per human and sends one while that human's turn is active. The AI's turn stays active until the leader screen closes. The human answers with the `*_RESPONSE` events, `DEMAND_HUMAN_REFUSAL`, `REQUEST_HUMAN_REFUSAL` or by accepting the AI's deal.

## What the headless runtime had

- `rt_diplo` received every leader message, answered the AI's requests with the human's "no" and dismissed the rest (`SIMP-003`). It reported the declined ones as `GC_EVENT_DIPLO_AUTO_DECLINED`; the plain replies were never reported.
- The trade functions of the classic client's UI (`GcUiProposeDeal`, `GcUiEqualize` and others) work on one scratch deal and the active player: a single-seat replica's API, unusable for several seats on one server.
- Seat-scoped read-only queries existed for current, historic and proposed deals and for advisor counsel. The read scope (`GcUiReadScope`) puts back what a Lua getter may change.
- The delivery of `sendNetDealAccepted` and `sendFromUIDiploEvent` already ran as the sender (`ActiveAs`), so replies were built for the right human.

## Server design

**Leader session per seat.** Every seat has at most one leader session (`_lib.lua`): the AI it talks to, the screen mode, the leader's last line, the refused-offer count and, per turn and partner, the open count for the repeat greetings. It is screen state of the worker's Lua, like a draft: it is never saved, never journaled and never changes the game. It plays the part of the original's local leader screen, one per seat instead of one per machine.

**Queries (read-only, in the read scope).** All take the seat explicitly, switch the active player to the seat for the call only (`ActiveAs` changes the pregame field and nothing else) and use a local deal, never the shared scratch deal.

| Function | Original |
| --- | --- |
| `GcUiLeaderGreeting(seat, ai, count)` | `GetGreetHumanMessage` with the seat's own open count |
| `GcUiDiploText(seat, ai, name)` | `GetDiploStringForMessage` for a named line |
| `GcUiDealQuery(seat, partner, items, "check")` | the trade screen adding the items in order |
| `GcUiDealQuery(..., "pockets")` | what each side may still add (`IsPossibleToTradeItem`) |
| `GcUiDealQuery(..., "evaluate")` | the verdict of `DoHumanOfferDealToThisAI` |
| `GcUiDealQuery(..., "equalize", mode)` | `DoEqualizeDealWithHuman` on a copy |
| `GcUiDealQuery(..., "blob")` | the deal as `sendNetDealAccepted` carries it |

**Commands (ordered, journaled).** Everything that changes the game is the seat's own net message, submitted by the screen action as an ordinary order and delivered by the core as the sender:

- a discussion topic, a war together or a denunciation: `sendFromUIDiploEvent(ai, event, arg1, arg2)`;
- an accepted offer: `sendNetDealAccepted(ai, value, mine, theirs)` with the core's own deal blob (`net.send_deal`, lowercase hex of whole bytes, at most 8192 digits, only on deal messages);
- war: the existing `declare_war` window.

Talk and Discuss send nothing: they only produce the leader's line, as in the original (`HUMAN_WANTS_DISCUSSION` changes no state, so the line is read instead of sent).

**Replies.** The core now reports every leader message as `GC_EVENT_DIPLO_LEADER_MESSAGE` (AI, state, human, animation, data, request flag, answer; the line). `RealCore::leader_messages` drains them and the worker hands each one to the seat's session (`__leader_message`). A line from another AI, or for a seat without a session, is dropped. The state picks the mode: the human-invoked discussion stays in the discussion menu, every other line returns to the leader's root.

**Determinism.** Queries and actions run in the read scope; only their net messages change the game, after the action returned, in the order the worker applies orders. Each order is pumped before the next one is taken, so a verdict is always made on the state the deal is enacted in. The core's `FinalizeDeal` checks every item again when the deal is applied. The real-core test checks that talking, discussing, drafting and asking leave the simulation digest unchanged.

**Multiplayer.** Two humans talking to the same AI in one turn have separate sessions, drafts and counts. Their messages are ordered by the worker; each reply is addressed to the human whose message caused it (the delivery runs with that human active), so it reaches that seat's session only. No protocol change: the screens use `Query::Screen`, `Command::ScreenAction` and the existing window transitions.

## Status

Done:

- Talk opens the leader window with the AI's greeting and mood, opinion reasons and relations (`diplo.begin_diplo`, `leader`).
- The discussion menu: friendship, denounce (confirmed), do not settle, stop spying, stop converting, stop digging (hidden once asked), a war together with a target list, never mind. The AI's replies come back to the seat.
- Trade with an AI: the core's pockets for both sides (gold, gold per turn, luxury and strategic resources, embassy, open borders, defensive pact, research agreement), the table checked item by item, "what would make this work" in its three forms, propose with the AI's verdict and the leader's line, and an accepted deal enacted as the seat's order. Closing the table returns to the leader.

Remaining:

- Peace talks (`make_peace`): `HUMAN_NEGOTIATE_PEACE`, the AI's peace price and the peace treaty on the table. The AI's peace value is cached per AI (`m_iCachedValueOfPeaceWithHuman`), so two seats negotiating peace with one AI in one turn need a per-seat value first.
- Demands of an AI (`DoHumanDemand` draws from the asynchronous random numbers on the asker's machine).
- AI-initiated requests and messages as popups answered by the seat: the core must keep them for seats that answer them, as it does for the move war question, instead of declining them (`SIMP-003`). Their trade screens (AI offer, demand, request, concessions) and the `TRADE_SCREEN_CLOSED` event belong here.
- Cities, other civilizations' wars and peace, and votes on the AI table; proposing a deal to a human.

## Simplifications

- `SIMP-003` (core): the AI's own requests are still declined by the core; the leader screen's `respond` has nothing to answer.
- `SIMP-456`: the AI table offers gold, income, resources, embassies, open borders, defensive pacts and research agreements; a human draft cannot be proposed.
- `SIMP-457`: no demands and no peace talks with an AI yet.
- `SIMP-450`: deal history omits the item kinds without a model kind.

## Tests

```sh
cargo test -p game-host --test leader_script --test deal_script --test diplo_script
cargo test -p game-host screens::runtime::tests::send_deal
cargo test -p gamecore event
# real core (Linux):
cargo test -p game-host --test real_ai_diplomacy -- --test-threads=1
cargo test -p game-host --test real_screen_purity -- --test-threads=1
```
