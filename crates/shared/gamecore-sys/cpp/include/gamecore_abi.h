/* gamecore_abi.h: narrow C ABI v2 between the game core ("brain", C++) and its host (Rust).
 * C89-clean apart from <stdint.h>.
 * v2 (P1): several humans, the server seat, per-player commands and queries, slot control. The header is the full v2
 * contract; the entries and kinds the later packages (P13: the data for the render track, ...) build are declared here and answer
 * GC_ERR_INVALID_ARG (unknown kind) / NOT_IMPLEMENTED until their package lands. P8 (gc_slot_control, N3.12, N3.13) and P9 (MAKE_PEACE, Q_DIPLO, N3.10) are built.
 *
 * Rules
 *  - One gc_instance per process, one game per instance (gc_new_game or gc_load_game, once).
 *  - Calls and host callbacks run on one thread. Nothing throws across the ABI: every error is a return
 *    code and, when the call takes a gc_error*, a message (English, NUL-terminated, truncated to fit).
 *  - Host callbacks must not throw and must not call gc_*.
 *  - Changes are append-only (new functions, error codes, kinds, struct fields at the end); a breaking
 *    change bumps GC_ABI_VERSION. The civ5-reimagened server keeps a verbatim copy of this file (gamecore-sys).
 *
 * Staging in this revision (F2): lifecycle (create, new_game, pump, state_digest, destroy), buffers,
 * errors and events work, gc_query answers Q_GAME, Q_PLOTS, Q_UNITS, Q_PLAYERS, Q_UNIT_ACTIONS, Q_PATH,
 * Q_REACHABLE and Q_CATALOG, and gc_apply_command takes the unit commands 1..12 and END_TURN / FORCE_END_TURN.
 * gc_save and gc_load_game work (G4a). The other command and query kinds and gc_lua_register are declared
 * and validate their arguments, then return GC_ERR_NOT_IMPLEMENTED until their tasks land (later
 * commands and queries, Lua bridge). */
#ifndef GAMECORE_ABI_H
#define GAMECORE_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GC_ABI_VERSION 2u

/* ---- return codes ------------------------------------------------------------------------------
 * 0 is success, everything else is negative. gc_error.code repeats the return code. */
#define GC_OK 0
#define GC_ERR_ALREADY_EXISTS (-1) /* an instance exists / a game was already started in this instance */
#define GC_ERR_INVALID_ARG (-2)    /* NULL or out-of-range argument; unknown or ill-typed setup key */
#define GC_ERR_WRONG_MODE (-3)     /* reserved (v0 replica mode, removed); never returned */
#define GC_ERR_BAD_DATA (-4)       /* malformed or incompatible save data */
#define GC_ERR_REJECTED (-5)       /* the game refused a command; message says why */
#define GC_ERR_NOT_STARTED (-6)    /* no game yet (gc_new_game / gc_load_game not done) */
#define GC_ERR_NOT_FOUND (-7)      /* host read_file: no such file */
#define GC_ERR_STUCK (-8)          /* the turn cannot continue without UI (gc_pump) */
#define GC_ERR_NOT_IMPLEMENTED (-9) /* declared, not built yet in this revision */
#define GC_ERR_DIVERGED (-10)     /* B4: gc_journal_apply: the replica left the server's game (a refused input, another pump result, a digest); resync it */
#define GC_ERR_INTERNAL (-100)     /* boot or runtime failure, or a caught C++ exception; see message */

/* ---- log levels (gc_host.log) ---- */
#define GC_LOG_ERROR 0
#define GC_LOG_WARN 1
#define GC_LOG_INFO 2
#define GC_LOG_DEBUG 3
#define GC_LOG_TRACE 4

typedef struct gc_instance gc_instance;
struct lua_State;

/* ---- buffers ----------------------------------------------------------------------------------
 * Whoever fills a gc_buf sets release (may be NULL for static data). The receiver calls
 * gc_buf_release exactly once. Library -> host buffers (gc_save, gc_query) are owned by the host
 * after the call; host -> library buffers (read_file) are owned by the library after the call.
 * Input pointers (setup_json, save bytes, args, command) are only read during the call. */
typedef struct gc_buf {
    uint8_t* data;
    size_t len;
    void (*release)(struct gc_buf* self);
    void* owner;
} gc_buf;

/* Calls release if set, then zeroes buf. NULL-safe, safe on a zeroed buffer. */
void gc_buf_release(gc_buf* buf);

/* v2: reason (appended) says why a command or slot operation was refused, as a code the host can act on (GC_REASON_*,
 * section 4.4; the message stays the text for people). 0 (GC_REASON_NONE) on success and on errors without a reason. */
#define GC_REASON_NONE 0
#define GC_REASON_NOT_HUMAN 1     /* the player is not a human of this game */
#define GC_REASON_NOT_YOUR_TURN 2 /* the player's turn is not active now (hybrid: another one's turn) */
#define GC_REASON_TURN_ENDED 3    /* the player has already ended the turn */
#define GC_REASON_AI_RUNNING 4    /* the AI is moving; the command may pass after it */
#define GC_REASON_UNIT_BUSY 5     /* the unit or city is busy or dying */
#define GC_REASON_BAD_TARGET 6    /* target off the map, unreachable, not an enemy */
#define GC_REASON_RULE 7          /* the rules forbid it (no city here, no gold, no peace possible) */
#define GC_REASON_BLOCKED 8       /* the end of turn is held by a blocker (named in the message and in Q_TURN) */
#define GC_REASON_PUMP_FIRST 9    /* an undelivered command or a busy unit: gc_pump first */
#define GC_REASON_NOT_SAFE_POINT 10 /* only allowed at a safe point */
#define GC_REASON_SLOT_STATE 11   /* the slot operation does not fit the slot's state */

typedef struct gc_error {
    int32_t code;
    char message[256];
    int32_t reason; /* GC_REASON_* */
} gc_error;

/* ---- events (host callback, section 4.7) ---------------------------------------------------------
 * args, text and text2 are valid only during the callback. text/text2 are UTF-8 or NULL. */
typedef struct gc_event {
    int32_t kind;
    uint32_t argc;
    const int32_t* args;
    const char* text;
    const char* text2;
} gc_event;

#define GC_EVENT_TURN_STARTED 1        /* args: turn. Emitted by gc_pump for every game-turn change */
#define GC_EVENT_PLAYER_TURN_STARTED 2 /* args: player */
#define GC_EVENT_NOTIFICATION_ADDED 3  /* args: id, type, x, y, data1, data2; text: message, text2: summary */
#define GC_EVENT_NOTIFICATION_REMOVED 4 /* args: id */
#define GC_EVENT_AUTO_CHOICE 5         /* args: choice_kind, player, item; text: what was chosen */
/* AUTO_CHOICE choice_kind (F3, F4, F5, SIMP-002; append only): 0 research, 1 free tech, 2 policy / tenet, 3 policy branch, 4 ideology,
 * 5 pantheon, 6 religion founded, 7 religion enhanced, 8 reformation belief, 9 promotion, 10 free great person, 11 great person bought with faith,
 * 12 Maya Long Count great person, 13 captured city (annexed / puppeted / razed), 14 World Congress proposals, 15 World Congress votes,
 * 16 archaeology (finished dig), 17 ruin reward (goody hut), 18 tech steal by a spy;
 * item = TechTypes / PolicyTypes / PolicyBranchTypes (the ideology: its branch) / BeliefTypes (5, 8) / ReligionTypes (6, 7) / PromotionTypes (9) /
 * UnitTypes (10, 11, 12) / the city id (13) / the id of the first new proposal (14) / the number of delegates cast (15) / ArchaeologyChoiceType (16) /
 * GoodyTypes (17) / TechTypes (18); text = the English journal line ("Researching Pottery (chosen automatically)"). */
#define GC_EVENT_DIPLO_AUTO_DECLINED 6 /* args: ai_player, kind; text */
#define GC_EVENT_POPUP_INFO 7          /* args: popup_type, data1, data2, data3; text */
#define GC_EVENT_GAME_OVER 8           /* args: winner_team, victory_type. Emitted once by gc_pump */
#define GC_EVENT_LUA_ERROR 9           /* text: message and place */
#define GC_EVENT_WAR_STATE 10          /* args: team_a, team_b, at_war */
#define GC_EVENT_MESSAGE 11            /* args: player, x, y, message_type, source, force; text: the message (localized, markup like [COLOR_..] / [NEWLINE] kept) */
/* MESSAGE (G1, append only): the core's own transient info / combat lines (CvDLLInterface AddMessage / AddUnitMessage / AddPlotMessage; the
 * engine UI shows them as flying text and in the message log). Only messages addressed to the active player (the human) are emitted; a
 * message for another player is dropped as the engine UI would not show it, and so are empty texts. x / y: the plot of the message, -1 when it
 * has none (AddMessage carries a plot only when the caller passed iFlashX / iFlashY, the combat lines do not; AddUnitMessage: the unit's plot
 * when it still exists; AddPlotMessage: the plot). message_type = InterfaceMessageTypes (0 INFO, 1 DISPLAY_ONLY, 2 MAJOR_EVENT, 3 MINOR_EVENT,
 * 4 CHAT, 5 COMBAT_MESSAGE, 6 QUEST). source: 0 AddMessage, 1 AddUnitMessage, 2 AddPlotMessage. force = the caller's bForce (0 / 1). */

/* ---- v2 events (section 4.6; append only, args only grow at the end: an old reader skips the extra) ----
 * v2 also appends args to events 2, 3, 6, 7 and 11 (the human the event is for) and makes MESSAGE go to every human addressee:
 *  2 PLAYER_TURN_STARTED: for every human, args: player (unchanged shape)
 *  3 NOTIFICATION_ADDED: args: id, type, x, y, data1, data2, player
 *  4 NOTIFICATION_REMOVED: args: id, player
 *  6 DIPLO_AUTO_DECLINED: args: ai_player, kind, human (who was offered)
 *  7 POPUP_INFO: args: popup_type, data1, data2, data3, player (the active player at the moment of the popup)
 * 11 MESSAGE: args as in v1; emitted for any human addressee, not only the active player. */
#define GC_EVENT_PLAYER_TURN_ENDED 12  /* args: player, ended (1 sent the end of turn, 0 cancelled it) */
#define GC_EVENT_SLOT_CHANGED 13       /* args: player, slot_status (SlotStatus), connected, kicked; text: the nick */
#define GC_EVENT_HUMAN_PHASE_STARTED 14 /* args: turn. The AI civs are done, the humans may act (autosave point of the turn start) */
#define GC_EVENT_HUMAN_WAR_DECLARED 15 /* args: from_team, to_team, extends_timer (1 when a side of the declaration moves simultaneously now) */
#define GC_EVENT_PLAYER_TURN_STATUS 16 /* args: status (0 = setTurnActive(true), 1 = setTurnActive(false)), player (any, AI and minors included), human (1 when the player is a human). Not sent by a replica. */

/* gc_host is copied by gc_create; user must outlive the instance.
 * read_file returns a whole file from the game-data layout (db/gameplay.db, db/loc-en_us.db,
 * vfs-index.json, Lua and map scripts, ...) or GC_ERR_NOT_FOUND. */
typedef struct gc_host {
    void* user;
    void (*log)(void* user, int32_t level, const char* msg);
    int32_t (*read_file)(void* user, const char* vfs_path, gc_buf* out);
    void (*game_event)(void* user, const gc_event* ev);
} gc_host;

/* ---- pump (section 4.3) ---- */
#define GC_PUMP_WAITING_HUMAN 0 /* the human is up and the AI civs are done; blocker says what holds the turn */
#define GC_PUMP_AI_RUNNING 1    /* the slice budget ran out; call gc_pump again */
#define GC_PUMP_GAME_OVER 2
#define GC_PUMP_STUCK 3         /* gc_pump also returns GC_ERR_STUCK, message has the reason */

typedef struct gc_pump_result {
    int32_t turn;          /* game turn after the call */
    int32_t status;        /* GC_PUMP_* */
    int32_t active_player; /* who is moving now (the human), -1 = none */
    int32_t blocker;       /* EndTurnBlockingTypes of the human, -1 = none */
    int32_t slices_used;   /* slices run by this call; deterministic, independent of the clock */
    int32_t humans_left;   /* v2: humans the turn still waits for (WAITING_HUMAN: who, see Q_TURN); 0 while the AI moves */
} gc_pump_result;

/* ---- commands (section 4.4; F2) ---- */
#define GC_CMD_UNIT_MOVE_TO 1        /* unit, x, y */
#define GC_CMD_UNIT_RANGE_ATTACK 2   /* unit, x, y */
#define GC_CMD_UNIT_FOUND_CITY 3     /* unit */
#define GC_CMD_UNIT_FORTIFY 4        /* unit */
#define GC_CMD_UNIT_SLEEP 5          /* unit */
#define GC_CMD_UNIT_SKIP 6           /* unit */
#define GC_CMD_UNIT_HEAL 7           /* unit */
#define GC_CMD_UNIT_AUTOMATE_BUILD 8 /* unit */
#define GC_CMD_UNIT_AUTOMATE_EXPLORE 9 /* unit */
#define GC_CMD_UNIT_CANCEL 10        /* unit */
#define GC_CMD_UNIT_DISBAND 11       /* unit */
#define GC_CMD_UNIT_WAKE 12          /* unit */
#define GC_CMD_CITY_SET_PRODUCTION 20 /* city, order_type, item */
#define GC_CMD_CITY_PURCHASE 21      /* city, order_type, item, yield */
#define GC_CMD_CITY_RANGE_ATTACK 22  /* city, x, y */
#define GC_CMD_DECLARE_WAR 30        /* team; v2: also against the team of another human */
#define GC_CMD_MAKE_PEACE 31         /* v2: team. White peace between two humans who both agreed (the agreement lives in the host) */
#define GC_CMD_END_TURN 40
#define GC_CMD_FORCE_END_TURN 41
#define GC_CMD_CANCEL_END_TURN 42    /* v2: no args. Takes back the player's end of turn while not every human has ended the turn */
#define GC_CMD_END_TURN_TIMEOUT 43   /* v2: no args. Server only: the turn timer ran out; ends the player's turn without the blocker checks */

/* v2: player is the human the command is for (any seated human; the check is HumanMayAct(player), refusals carry GC_REASON_*). */
typedef struct gc_command {
    uint16_t kind;
    uint8_t player;
    uint8_t reserved;
    uint32_t argc;
    const int32_t* args;
    const char* text;
} gc_command;

/* ---- queries (section 4.5; E2) ---- */
#define GC_Q_GAME 1
#define GC_Q_PLOTS 2
#define GC_Q_UNITS 3
#define GC_Q_CITIES 4
#define GC_Q_PLAYERS 5
#define GC_Q_TOP_BAR 6
#define GC_Q_CITY_BUILDS 7
#define GC_Q_UNIT_ACTIONS 8
#define GC_Q_PATH 9
#define GC_Q_REACHABLE 10
#define GC_Q_RANGE_TARGETS 11
#define GC_Q_NOTIFICATIONS 12
#define GC_Q_CATALOG 13
#define GC_Q_TURN 14             /* v2: no args; the turn status of every human slot */
#define GC_Q_DIPLO 15            /* v2: args {player} */
#define GC_Q_REACHABLE_TREE 16   /* v2: args {player, unit id [, horizon 1..3, default 1]} */
#define GC_Q_CITY_ART 17         /* v2, preliminary (4.14): args {viewer_team}; the record is fixed by package P13 */
#define GC_Q_TRADE_ROUTES 18     /* v2, preliminary (4.14): args {viewer_team}; the record is fixed by package P13 */
#define GC_Q_PLOT_BASIS 19       /* v2: args {} = the whole map, or {plot index, ...} = those plots; no viewer_team: the base is not fogged */
#define GC_Q_COMBAT_PREVIEW 20   /* v2: args {player, attacker, x, y}; attacker >= 0 = a unit id, < 0 = -(city id) (as Q_RANGE_TARGETS) */
#define GC_Q_PLOTS_DIRTY 21      /* v2: no args; plot indices changed since the last call (the call resets the set) */
#define GC_Q_TRADE_PATHS 22      /* B4 fix-path: args {viewer_team}; the plots of the routes Q_TRADE_ROUTES lists */

/* Every gc_query reply starts with this header (little endian), then count * record_size bytes, then
 * the string table (UTF-8, NUL-terminated strings, referenced by u32 offset; 0xFFFFFFFF = none).
 * strings_offset = sizeof(gcq_header) + count * record_size. Fields are only ever appended to a record
 * (record_size grows); a reader skips what it does not know. Padding bytes are zero. */
typedef struct gcq_header {
    char magic[4]; /* "GCQ1" */
    uint16_t format; /* 1 */
    uint16_t kind;
    uint32_t count;
    uint32_t record_size;
    uint32_t strings_offset; /* from the start of the buffer */
    uint32_t strings_size;
} gcq_header;

#define GCQ_NO_STRING 0xFFFFFFFFu

/* Q_GAME: no args, one record. v2: args {} (human_player = the first human slot, for hosts of v1) or {player} (blocker, blocker_*, and
 * active_player describe that player). v2 appends the fields after reserved[2] (record 56 bytes):
 * game_type = GameTypes of the core (0 single, 1 network); turn_mode GC_TURN_MODE_*; phase GC_PHASE_*; humans = seated humans;
 * humans_left = humans the turn waits for; turn_len_s = the length of this turn's timer in seconds, as the exe enforces it (Lekmod's once per
 * turn: CvGame::getPreviousTurnLen(), set at the turn start from getMaxTurnLen(); 0 = no timer, and on game turn 0, where the exe runs none);
 * turn_base_s = the base time of the chosen TurnTimers row (0 = no timer chosen); map_seed = the pregame map seed (preliminary, 4.14); safe_point = 1 at a safe point (4.9);
 * unsafe_reason GC_UNSAFE_*. */
#define GC_TURN_MODE_SIMULTANEOUS 1
#define GC_TURN_MODE_HYBRID 2
#define GC_PHASE_HUMANS 0
#define GC_PHASE_AI 1
#define GC_UNSAFE_NONE 0
#define GC_UNSAFE_AI_RUNNING 1       /* the AI is computing */
#define GC_UNSAFE_COMMAND_GIVEN 2    /* a command was given after WAITING_HUMAN */
#define GC_UNSAFE_AI_REQUEST_OPEN 3  /* an AI request to a human is open */
#define GC_UNSAFE_AUTOMOVES 4        /* a human is in auto moves */
#define GC_GAME_STATUS_WAITING_HUMAN 0 /* = GC_PUMP_*; AI_RUNNING until the first gc_pump has finished a call */
#define GC_GAME_STATUS_AI_RUNNING 1
#define GC_GAME_STATUS_GAME_OVER 2
#define GC_GAME_STATUS_STUCK 3
typedef struct gcq_game {
    int32_t turn;
    int32_t year;          /* calendar year (negative = BC) */
    int32_t blocker_unit;  /* unit id of the blocker's target, -1 = none */
    int32_t blocker_city;  /* city id of the blocker's target, -1 = none */
    int16_t blocker_x;     /* plot of the target, -1 = none */
    int16_t blocker_y;
    int16_t map_width;
    int16_t map_height;
    int8_t human_player;   /* the active (human or observer) slot, -1 = none */
    int8_t human_team;
    int8_t active_player;  /* who is moving now (the human once the AI civs are done), -1 = none */
    int8_t status;         /* GC_GAME_STATUS_* */
    int8_t blocker;        /* EndTurnBlockingTypes of the human, -1 = none (live, not the last pump's) */
    uint8_t wrap_x;
    uint8_t wrap_y;
    int8_t difficulty;     /* HandicapTypes of the game */
    int8_t winner;         /* winning team, -1 = none */
    int8_t victory_type;   /* VictoryTypes, -1 = none */
    uint8_t reserved[2];
    int8_t game_type;      /* v2 */
    int8_t turn_mode;
    int8_t phase;
    uint8_t humans;
    uint8_t humans_left;
    int8_t safe_point;
    int8_t unsafe_reason;
    uint8_t reserved1;
    int32_t turn_len_s;
    int32_t turn_base_s;
    uint32_t map_seed;
} gcq_game;                /* 56 bytes (v1: 36) */

/* Q_PLOTS: args = {viewer_team}; viewer_team -1 = the whole truth (everything revealed and visible).
 * count = map_width * map_height, rows by y, then x. An unrevealed plot has x and y and nothing else (all zero:
 * test GC_PLOT_F_REVEALED before reading terrain 0 / owner 0). A revealed plot that is not visible shows the
 * last seen owner, improvement and route (-1 = none) and no live data (no city, pillaged flag, yields).
 * v2: args {viewer_team [, plot index, ...]}: with indices only those plots, in the order given (a bad index is GC_ERR_INVALID_ARG). */
#define GC_PLOT_F_REVEALED 0x0001u
#define GC_PLOT_F_VISIBLE 0x0002u
#define GC_PLOT_F_CITY 0x0004u
#define GC_PLOT_F_LAKE 0x0008u
#define GC_PLOT_F_COASTAL 0x0010u
#define GC_PLOT_F_RIVER_SHIFT 5 /* 6 bits: bit (5 + d) = river on the edge towards DirectionType d (0 NE, 1 E, 2 SE, 3 SW, 4 W, 5 NW) */
#define GC_PLOT_F_RIVER_MASK 0x07E0u
#define GC_PLOT_F_PILLAGED 0x0800u
#define GC_PLOT_F_NATURAL_WONDER 0x1000u
#define GC_PLOT_F_WORKED 0x2000u         /* v2 (P13): a city works the plot (CvPlot::isBeingWorked); set only on a plot the viewer sees now and whose owner is on the viewer's team (viewer -1: any worked plot) */
#define GC_PLOT_F_ROUTE_PILLAGED 0x4000u /* v2 (P13): the route is pillaged (CvPlot::IsRoutePillaged); visible plots only, like GC_PLOT_F_PILLAGED */
#define GC_PLOT_TYPE_MOUNTAIN 0 /* PlotTypes */
#define GC_PLOT_TYPE_HILLS 1
#define GC_PLOT_TYPE_LAND 2
#define GC_PLOT_TYPE_OCEAN 3
typedef struct gcq_plot {
    int16_t x;
    int16_t y;
    uint16_t flags;
    int8_t plot_type;     /* PlotTypes */
    uint8_t reserved0;
    int16_t terrain;      /* TerrainTypes */
    int16_t feature;      /* -1 = none */
    int16_t resource;     /* -1 = none or not known to the viewer */
    int8_t resource_qty;
    uint8_t reserved1;
    int16_t improvement;  /* last seen, -1 = none */
    int16_t route;        /* last seen, -1 = none */
    int8_t owner;         /* last seen player, -1 = none */
    int8_t continent_art; /* CvPlot::GetContinentType, the art style of the continent */
    int8_t yields[6];     /* food, production, gold, science, culture, faith; visible plots only */
    int16_t build_type;   /* v2 (P13): BuildTypes of the most advanced job under way on the plot (CvPlot::getBuildProgress), -1 = none; visible plots only */
    uint8_t build_pct;    /* v2 (P13): its progress, 0..100 (progress * 100 / the build time for the plot's owner); 0 when build_type is -1 */
    uint8_t reserved2;
} gcq_plot;               /* 32 bytes (v1: 28) */

/* Q_PLAYERS: args = {viewer_team}. One record per slot that ever lived (majors, city-states, barbarians).
 * A player the viewer has not met: civ_type and leader_type -1, names "Unknown"; id, team, colors (borders
 * are drawn with them) and the alive / minor / barbarian flags are still set. viewer_team -1 = all met.
 * v2 appends nick (string of the human's nick, GCQ_NO_STRING for a slot without one), slot (SlotStatus of the core: 0 OPEN, 1 COMPUTER,
 * 2 CLOSED, 3 TAKEN, 4 OBSERVER), GC_PLAYER_F_OBSERVER (the server seat: the host does not list it) and art_techs (bit i = the player has the
 * i-th technology of the landmark art list that Q_CATALOG gives; only for met players). */
#define GC_PLAYER_F_ALIVE 0x0001u
#define GC_PLAYER_F_HUMAN 0x0002u
#define GC_PLAYER_F_MINOR 0x0004u
#define GC_PLAYER_F_BARBARIAN 0x0008u
#define GC_PLAYER_F_MET 0x0010u
#define GC_PLAYER_F_AT_WAR_WITH_VIEWER 0x0020u
#define GC_PLAYER_F_OBSERVER 0x0040u
typedef struct gcq_player {
    int8_t id;
    int8_t team;
    int16_t civ_type;
    int16_t leader_type;
    uint16_t flags;
    uint32_t color_primary;   /* 0xRRGGBBAA */
    uint32_t color_secondary;
    uint32_t name;            /* string: leader or city-state name */
    uint32_t civ_name;        /* string: civilization short name */
    int8_t era;               /* appended (B1): CvPlayer::GetCurrentEra (EraTypes), -1 = not met */
    uint8_t reserved0[3];
    uint32_t nick;            /* v2: string */
    int8_t slot;              /* v2: SlotStatus */
    uint8_t reserved1[3];
    uint32_t reserved2;       /* keeps art_techs on an 8-byte offset on every target */
    uint64_t art_techs;       /* v2 */
} gcq_player;                 /* 48 bytes (v1: 28) */

/* Q_CATALOG: args = {table}. Static reference data of one database table, one record per entry the core knows
 * (id = the core's type index, the same number every other query uses). Strings that do not apply are
 * GCQ_NO_STRING, icon_index / flag_index -1. */
#define GC_CATALOG_UNITS 0
#define GC_CATALOG_BUILDINGS 1
#define GC_CATALOG_PROJECTS 2
#define GC_CATALOG_TECHNOLOGIES 3
#define GC_CATALOG_POLICIES 4
#define GC_CATALOG_TERRAINS 5
#define GC_CATALOG_FEATURES 6
#define GC_CATALOG_RESOURCES 7
#define GC_CATALOG_IMPROVEMENTS 8
#define GC_CATALOG_ROUTES 9
#define GC_CATALOG_CIVILIZATIONS 10
#define GC_CATALOG_LEADERS 11
#define GC_CATALOG_ERAS 12
#define GC_CATALOG_HANDICAPS 13
#define GC_CATALOG_PROMOTIONS 14
#define GC_CATALOG_TABLE_COUNT 15
typedef struct gcq_catalog {
    int16_t id;
    int16_t icon_index;
    int16_t flag_index;
    int16_t reserved;
    uint32_t type;           /* string: "UNIT_WARRIOR" */
    uint32_t name;           /* string: localized (en_US) */
    uint32_t art_define;     /* string: ArtDefineTag / UnitArtInfo (Eras: ArtPrefix) */
    uint32_t icon_atlas;     /* string */
    uint32_t flag_atlas;     /* string (Units) */
    uint32_t art_style;      /* string (Civilizations: ArtStyleType; Eras: CityArtType) */
    uint32_t strategic_view; /* string, always GCQ_NO_STRING for now (no such column in the game data) */
} gcq_catalog;               /* 36 bytes */

/* Q_UNITS: args = {viewer_team}; viewer_team -1 = every unit. Own-team units always; other teams' units only on a plot the viewer
 * sees right now and not invisible to the viewer. Order: by owner, then by unit id. moves are in 1/60 of a move (the core's
 * MOVE_DENOMINATOR); strength / ranged are the base values (0 = none); activity is ActivityTypes (0 awake, 1 hold, 2 sleep, 3 heal,
 * 4 sentry, 5 intercept, 6 mission). */
#define GC_UNIT_F_CAN_MOVE 0x0001u      /* canMove(): moves left and not immobile */
#define GC_UNIT_F_NEEDS_ORDERS 0x0002u  /* awake, no orders queued, not automated (what the UNITS end-turn blocker waits for) */
#define GC_UNIT_F_FORTIFIED 0x0004u
#define GC_UNIT_F_SLEEPING 0x0008u      /* ACTIVITY_SLEEP and not fortified */
#define GC_UNIT_F_AUTOMATED 0x0010u
#define GC_UNIT_F_EMBARKED 0x0020u
#define GC_UNIT_F_CIVILIAN 0x0040u
#define GC_UNIT_F_CAN_PROMOTE 0x0080u
#define GC_UNIT_F_GARRISONED 0x0100u
#define GC_UNIT_F_CARGO 0x0200u         /* v2 (P13): the unit is carried by a transport (CvUnit::isCargo); transport_id says which */
typedef struct gcq_unit {
    int32_t id;          /* unique inside the owner */
    int8_t owner;
    uint8_t reserved0;
    int16_t unit_type;   /* UnitTypes */
    int16_t x;
    int16_t y;
    int16_t hp;
    int16_t max_hp;
    int16_t moves_left;
    int16_t max_moves;
    int16_t strength;
    int16_t ranged;
    int8_t range;
    int8_t domain;       /* DomainTypes */
    int8_t activity;
    int8_t level;
    int16_t experience;
    uint16_t flags;      /* GC_UNIT_F_* */
    uint32_t name;       /* string: localized name (a custom name when the unit has one) */
    int32_t transport_id; /* v2 (P13): id (inside the owner) of the transport that carries the unit, -1 = not carried; with GC_UNIT_F_CARGO */
    int16_t build_type;  /* v2 (P13): BuildTypes the unit is working on (CvUnit::getBuildType), -1 = none */
    int8_t facing;       /* v2 (P13): DirectionTypes the unit faces (CvUnit::getFacingDirection) */
    uint8_t reserved1;
} gcq_unit;              /* 44 bytes (v1: 36) */

/* Q_UNIT_ACTIONS: args = {player, unit id}. One record per unit command (GC_CMD_UNIT_* 1..12) that applies to the unit, in
 * command order; enabled says whether it can be given now (target-less check: UNIT_MOVE_TO / UNIT_RANGE_ATTACK only test the unit,
 * the target is checked by gc_apply_command). reason is the refusal text when enabled is 0, GCQ_NO_STRING otherwise. */
typedef struct gcq_unit_action {
    int16_t action;      /* GC_CMD_UNIT_* */
    uint8_t enabled;
    uint8_t reserved;
    uint32_t reason;     /* string */
} gcq_unit_action;       /* 8 bytes */

/* Q_PATH: args = {player, unit id, x, y}. The gamecore's interface pathfinder (the one behind the mouse-over path line) from the
 * unit's plot to the target, first node = the unit's plot. turn = turns from now at which the unit stands there: 0 = this turn.
 * No path (or the target is the unit's own plot) = zero records. A bad unit or an off-map target is GC_ERR_INVALID_ARG.
 * v2: the player may be any seated human (the unit must be the player's); the node gets flags (GC_REACH_F_*); a target that cannot be reached
 * is not an empty reply but one node, the target, with GC_REACH_F_UNREACHABLE. */
#define GC_REACH_F_ATTACK 0x0001u      /* the step attacks (an enemy stands there) */
#define GC_REACH_F_EMBARK 0x0002u      /* the unit embarks on this step */
#define GC_REACH_F_DISEMBARK 0x0004u   /* the unit disembarks on this step */
#define GC_REACH_F_CITY 0x0008u        /* the plot holds a city */
#define GC_REACH_F_STOP 0x0010u        /* the unit stops here (ZOC or the end of its moves) */
#define GC_REACH_F_UNREVEALED 0x0020u  /* the plot is not revealed to the unit's team */
#define GC_REACH_F_CAPTURE 0x0040u     /* the step captures a city or a civilian */
#define GC_REACH_F_UNREACHABLE 0x0080u /* Q_PATH only: the target has no path */
typedef struct gcq_path_node {
    int16_t x;
    int16_t y;
    int16_t turn;
    int16_t moves_left;  /* moves left on arrival, 1/60 of a move */
    uint16_t flags;      /* v2: GC_REACH_F_* */
    uint16_t reserved;
} gcq_path_node;         /* 12 bytes (v1: 8) */

/* Q_REACHABLE: args = {player, unit id}. The plots the unit can reach with the moves it has left this turn (not its own plot), by
 * y then x, same record as Q_PATH (turn is 0, moves_left on arrival). */

/* Q_RANGE_TARGETS (G1b): args = {player, shooter}; shooter >= 0 = a unit id of the player, shooter < 0 = -(city id) of the player
 * (the core's city ids are never 0). One record per plot the shooter can hit with a ranged attack right now, by y then x: exactly
 * the targets GC_CMD_UNIT_RANGE_ATTACK / GC_CMD_CITY_RANGE_ATTACK would accept (unit: CanStartMission(MISSION_RANGE_ATTACK) =
 * CvUnit::canRangeStrikeAt, at war with the target; city: CvCity::canRangeStrikeAt, units only). A shooter that cannot shoot now
 * (no attack left, embarked, busy, a city that already shot or is not on its turn) has zero records. A player out of range or a
 * unit / city the player does not have is GC_ERR_INVALID_ARG. */
#define GC_RANGE_TARGET_F_CITY 0x0001u /* the target plot holds a city (unit shooters only; cities never shoot at cities) */
typedef struct gcq_range_target {
    int16_t x;
    int16_t y;
    uint16_t flags;      /* GC_RANGE_TARGET_F_* */
    uint16_t reserved;
} gcq_range_target;      /* 8 bytes */

/* ---- G1a: cities, top bar, city production, city / war commands ---- */

/* OrderTypes of the core, as used by CITY_SET_PRODUCTION / CITY_PURCHASE and the production fields below (-1 = none). */
#define GC_ORDER_TRAIN 0     /* item = UnitTypes */
#define GC_ORDER_CONSTRUCT 1 /* item = BuildingTypes (wonders too) */
#define GC_ORDER_CREATE 2    /* item = ProjectTypes */
#define GC_ORDER_MAINTAIN 4  /* item = ProcessTypes (CITY_SET_PRODUCTION only); 3 (specialists) is not offered */
/* YieldTypes of the core: the 6 yield slots of Q_CITIES are in this order; CITY_PURCHASE pays with GOLD or FAITH. */
#define GC_YIELD_FOOD 0
#define GC_YIELD_PRODUCTION 1
#define GC_YIELD_GOLD 2
#define GC_YIELD_SCIENCE 3
#define GC_YIELD_CULTURE 4
#define GC_YIELD_FAITH 5

/* Q_CITIES: args = {viewer_team}; viewer_team -1 = every city. Order: by owner, then city id. The viewer's own cities
 * (all of them) carry live data. A foreign city is listed when its plot is visible to the viewer now (VISIBLE set; population,
 * hp, max_hp, defense and the CAPITAL / RAZING flags are live, the rest is as below) or when the plot is revealed and the owner last seen on it is the city's owner (last seen: VISIBLE clear, only
 * id, owner, x, y and name are set; population, hp, defense, flags and production are 0 / -1).
 * A city on an unrevealed plot is not listed. production_* describe the head of the order queue and are set for own
 * cities only (order / item -1 = nothing). production_turns / growth_turns -1 = never (no surplus). yields[6] (per turn,
 * GC_YIELD_* order; culture and faith are the city's own per-turn values) are set for own cities only.
 * defense = the city's combat strength (strength value / 100). */
#define GC_CITY_F_CAPITAL 0x0001u
#define GC_CITY_F_PUPPET 0x0002u          /* own only */
#define GC_CITY_F_RAZING 0x0004u
#define GC_CITY_F_OCCUPIED 0x0008u        /* own only: captured, not yet pacified by a courthouse */
#define GC_CITY_F_CAN_RANGE_ATTACK 0x0010u /* own only: canRangeStrike() and a target in range right now */
#define GC_CITY_F_VISIBLE 0x0020u         /* live data (see above); clear = last seen */
#define GC_CITY_F_RESISTANCE 0x0040u      /* own only */
typedef struct gcq_city {
    int32_t id;                /* unique inside the owner */
    int8_t owner;
    int8_t production_order;   /* GC_ORDER_*, -1 = none */
    int16_t x;
    int16_t y;
    int16_t population;
    uint16_t flags;            /* GC_CITY_F_* */
    int16_t hp;
    int16_t max_hp;
    int16_t defense;
    int16_t production_item;   /* -1 = none */
    int16_t production_turns;
    int16_t growth_turns;
    int16_t reserved;
    int32_t production_stored;
    int32_t production_needed;
    int16_t yields[6];
    uint32_t name;             /* string: localized city name */
    int8_t founder;            /* v2 (P13): CvCity::getOriginalOwner, -1 = unknown; set for own cities and cities seen now (GC_CITY_F_VISIBLE), -1 for a city last seen */
    uint8_t reserved3[3];
} gcq_city;                    /* 56 bytes (v1: 52) */

/* Q_CITY_ART (17, v2 package P13): args = {viewer_team}; viewer_team -1 = every city. One record per pair city - building for the buildings of the city that have art in the world
 * (Buildings.ArtDefineTag set: wonders, walls, the buildings with a model), ordered by owner, city id, building id. Own cities and cities seen now only (the cities Q_CITIES lists
 * with GC_CITY_F_VISIBLE or of the viewer's team); a city last seen has no records (the host keeps the last seen buildings). */
typedef struct gcq_city_art {
    int32_t city_id;           /* inside the owner, as gcq_city.id */
    int8_t owner;
    uint8_t reserved0;
    int16_t building;          /* BuildingTypes */
    int16_t x;                 /* the city's plot */
    int16_t y;
} gcq_city_art;                /* 12 bytes */

/* Q_TRADE_ROUTES (18, v2 package P13, Should): args = {viewer_team}; viewer_team -1 = every route. One record per trade route (CvGameTrade), in the core's order, for the routes of
 * the viewer's team (it owns the origin or the destination) and the routes with a plot the viewer sees now (any plot of the path, so a route crossing the viewer's land shows). */
typedef struct gcq_trade_route {
    int32_t id;                /* CvGameTrade's route id (m_iID) */
    int8_t owner;              /* the origin city's owner */
    int8_t dest_owner;
    int8_t domain;             /* DomainTypes: land or sea */
    uint8_t reserved0;
    int16_t from_x;            /* origin city plot */
    int16_t from_y;
    int16_t to_x;              /* destination city plot */
    int16_t to_y;
} gcq_trade_route;             /* 16 bytes */

/* Q_TRADE_PATHS (22, B4 fix-path): args = {viewer_team}, as Q_TRADE_ROUTES. The plots of each route Q_TRADE_ROUTES lists, in the same order: the route's
 * plot list (TradeConnection::m_aPlotList, the path of the DLL's trade route path finder that the caravan walks and the exe's TradeVisSystem draws), origin
 * city first, destination city last. A route of n plots is n consecutive records with its id. */
typedef struct gcq_trade_plot {
    int32_t route;             /* as gcq_trade_route.id */
    int16_t x;
    int16_t y;
} gcq_trade_plot;              /* 8 bytes */

/* Q_TOP_BAR: args = {player}; v2: any living major civ (v1: the human only), anything else is GC_ERR_INVALID_ARG. One record, values as
 * the top panel shows them. research_tech -1 = none chosen (the turns / progress / cost are then -1 / 0 / 0).
 * happiness = excess happiness (negative = unhappy). science_per_turn and gold_per_turn are the empire's net
 * rates. golden_age_turns = turns left of a running golden age, else 0. */
#define GC_TOPBAR_F_UNHAPPY 0x0001u
#define GC_TOPBAR_F_VERY_UNHAPPY 0x0002u
#define GC_TOPBAR_F_GOLDEN_AGE 0x0004u
typedef struct gcq_top_bar {
    int32_t gold;
    int32_t gold_per_turn;
    int32_t science_per_turn;
    int16_t research_tech;      /* TechTypes, -1 = none */
    int16_t research_turns;
    int32_t research_progress;
    int32_t research_cost;
    int32_t culture;
    int32_t culture_per_turn;
    int32_t next_policy_cost;
    int16_t happiness;
    int16_t golden_age_turns;
    int32_t faith;
    int32_t faith_per_turn;
    int16_t era;                /* EraTypes */
    uint16_t flags;             /* GC_TOPBAR_F_* */
    int32_t golden_age_progress;
    int32_t golden_age_threshold;
} gcq_top_bar;                  /* 60 bytes */

/* Q_CITY_BUILDS: args = {player, city id}; v2: the own city of any living major civ (v1: the human's). One record per thing the city could build (the production
 * popup's list: units, buildings and wonders, projects, processes), in that order, by item id. enabled = can be started
 * now; a listed but disabled item has a reason (the core's own text, tags stripped; GCQ_NO_STRING when enabled).
 * turns = turns to finish if started now at the current production (-1 = never, and for processes); production_cost
 * (0 for processes); gold_cost / faith_cost = cost to buy it right now (-1 = cannot be bought in this city).
 * can_purchase = a CITY_PURCHASE with gold would be accepted now (enough gold, the city may buy). */
#define GC_BUILD_F_WONDER 0x0001u    /* wonder, national wonder or other limited building */
#define GC_BUILD_F_CURRENT 0x0002u   /* the head of the order queue */
#define GC_BUILD_F_CAN_PURCHASE 0x0004u
#define GC_BUILD_F_ENABLED 0x0008u
typedef struct gcq_city_build {
    int8_t order_type;          /* GC_ORDER_* */
    uint8_t reserved0;
    int16_t item;
    int16_t turns;
    uint16_t flags;             /* GC_BUILD_F_* */
    int32_t production_cost;
    int32_t gold_cost;
    uint32_t reason;            /* string, GCQ_NO_STRING when enabled */
    int32_t faith_cost;
} gcq_city_build;               /* 24 bytes */

/* Q_NOTIFICATIONS (F7): args = {player}. The live notifications of that player, ascending by id, as the runtime captured them
 * from the core (AddNotification until RemoveNotification: a dismissed one is gone, so dismissed is 0 in every record). Only the
 * active (human) player's are broadcast, any other player's reply is empty (v2: read from the core's CvNotifications of any human). message / summary are localized strings
 * (the same text as gc_event.text / text2 of GC_EVENT_NOTIFICATION_ADDED); x / y are -1 when the notification has no plot;
 * type is the NotificationTypes hash; data1 / data2 are the core's game data / extra game data; turn is the game turn it was added. */
typedef struct gcq_notification {
    int32_t id;
    uint32_t type;
    int16_t x;
    int16_t y;
    int32_t data1;
    int32_t data2;
    int16_t turn;
    uint8_t dismissed;
    uint8_t reserved;
    uint32_t message;  /* string */
    uint32_t summary;  /* string */
} gcq_notification;    /* 32 bytes */

/* ---- v2: turn status, diplomacy, reach tree, plot base, combat preview, dirty plots (section 4.5) ---- */

/* Q_CATALOG, v2 tables (both served; GC_CATALOG_TABLE_COUNT still counts the 15 tables of v1, GC_CATALOG_TABLE_COUNT_V2 = 19 is the count to loop to,
 * a table from 19 on is GC_ERR_INVALID_ARG). Table 15 = the TurnTimers rows (record gcq_catalog_timer), table 16 = the landmark art technologies, in the
 * bit order of gcq_player.art_techs (at most 64; record gcq_catalog, only id, type and name are set). */
#define GC_CATALOG_TURN_TIMERS 15
#define GC_CATALOG_ART_TECHS 16
#define GC_CATALOG_WORLD_SIZES 17
#define GC_CATALOG_GAME_SPEEDS 18
#define GC_CATALOG_TABLE_COUNT_V2 19
/* Tables 17 and 18 (lobby lists of the Describe pass): table 17 = the Worlds rows by ID (record gcq_catalog_world_size), table 18 = the GameSpeeds rows by ID
 * (record gcq_catalog, only id, type and name are set). Tables 19 to 23 are the v3 tables below; a table from 24 on is GC_ERR_INVALID_ARG. */
typedef struct gcq_catalog_world_size {
    int16_t id;
    int16_t reserved;
    uint32_t type;           /* string: "WORLDSIZE_..." */
    uint32_t name;           /* string: Description */
    int16_t width;           /* GridWidth */
    int16_t height;          /* GridHeight */
    int16_t default_players; /* DefaultPlayers */
    int16_t default_minor;   /* DefaultMinorCivs */
} gcq_catalog_world_size;    /* 20 bytes */
typedef struct gcq_catalog_timer {
    int16_t id;
    int16_t reserved;
    uint32_t type;           /* string: "TURNTIMER_..." */
    uint32_t name;           /* string: localized */
    int32_t base_time;       /* CvTurnTimerInfo::getBaseTime() */
    int32_t city_resource;   /* getCityResource() */
    int32_t unit_resource;   /* getUnitResource() */
} gcq_catalog_timer;         /* 24 bytes */

/* Q_CATALOG, v3 tables (login-lobby packet P2: the staging room options of the game data; GC_CATALOG_TABLE_COUNT_V3 = 24 is the count to loop to, a table from 24 on
 * is GC_ERR_INVALID_ARG). Valid on a loaded session, before a game. Every record is a gcq_catalog_ext: four integers and four strings (GCQ_NO_STRING is never
 * used, an empty text is the empty string). Names and helps are localized (en_US; a text that is not a TXT_KEY_ is returned as it is).
 *   19 GAME_OPTIONS        every GameOptions row by ID. n = {id, default, visible, supports_multiplayer}, s = {type "GAMEOPTION_...", name (Description), help (Help), ""}
 *   20 VICTORIES           every Victories row by ID. n = {id, 0, 0, 0}, s = {type "VICTORY_...", name (Description), "", ""}
 *   21 MINOR_CIVS          every MinorCivilizations row by ID (its count is the upper end of the city-state slider). n = {id, 0, 0, 0}, s = {type, name (Description), "", ""}
 *   22 MAP_OPTIONS         every MapScriptOptions row (the custom options of each map script, as the exe stored them from GetMapScriptInfo().CustomOptions), by FileName and
 *                          then row order. n = {sort_priority, default_value, read_only, hidden}, s = {file_name (backslashes), option_id (the key of setup_json map_options and
 *                          of Map.GetCustomOption, "1", "2" ...), name, help (Description)}
 *   23 MAP_OPTION_VALUES   every MapScriptOptionPossibleValues row, by FileName, OptionID and SortIndex. n = {value (what map_options carries), sort_index, 0, 0},
 *                          s = {file_name, option_id, name, help}. An option with no value rows is a check box (0/1).
 * setup_json (gc_new_game), new keys: "victories": {"VICTORY_TIME":0, ...} (PreGame setVictory per Victories row; unlisted ones stay valid; an unknown type is an error) and
 * "max_turns": N (0..10000, PreGame setMaxTurns; 0 = the default of the game speed). */
#define GC_CATALOG_GAME_OPTIONS 19
#define GC_CATALOG_VICTORIES 20
#define GC_CATALOG_MINOR_CIVS 21
#define GC_CATALOG_MAP_OPTIONS 22
#define GC_CATALOG_MAP_OPTION_VALUES 23
#define GC_CATALOG_TABLE_COUNT_V3 24
/* v4 (b9 input-lobby P7): 24 CIV_INFO, one record per playable-or-not Civilizations row by ID, for the staging room's hover tip. n = {id, 0, 0, 0},
 * s = {type "CIVILIZATION_...", leader (the first Civilization_Leaders row's Description), traits (one line per trait of that leader, "Short name: description"),
 * unique (one line per unit or building the civilization overrides, "Unit: name" / "Building: name")}; lines are separated by "
". Localized (en_US). Valid before a game.
 * GC_CATALOG_TABLE_COUNT_V4 = 25 is the count to loop to. */
#define GC_CATALOG_CIV_INFO 24
#define GC_CATALOG_TABLE_COUNT_V4 25
typedef struct gcq_catalog_ext {
    int32_t n[4];
    uint32_t s[4];             /* strings */
} gcq_catalog_ext;             /* 32 bytes */

/* Q_TURN: args = {}. One record per slot that is or was a human, by player id: the turn status of everybody (and the input of the host's timer).
 * CONNECTED = the human is on line (gc_slot_control: set at the start of a game, cleared by DISCONNECT and KICK, never saved: after gc_load_game nobody is on line until
 * CONNECT); DISCONNECTED = Lekmod's autopause flag of a human who dropped while its turn was active (CvPlayer::disconnected); KICKED = the room host kicked the slot (an AI
 * plays it, until a CONNECT takes it again). The blocker's plot (blocker_x/y) is known only for a blocker the registry of the active player's notifications holds. */
#define GC_TURN_F_HUMAN 0x0001u        /* the slot is a human now */
#define GC_TURN_F_ALIVE 0x0002u
#define GC_TURN_F_CONNECTED 0x0004u
#define GC_TURN_F_DISCONNECTED 0x0008u /* Lekmod's autopause flag of a human who dropped */
#define GC_TURN_F_KICKED 0x0010u
#define GC_TURN_F_TURN_ACTIVE 0x0020u  /* isTurnActive() */
#define GC_TURN_F_TURN_ENDED 0x0040u   /* the end of turn is sent */
#define GC_TURN_F_CAN_END_TURN 0x0080u /* no blocker holds the end of turn */
#define GC_TURN_F_SIMULTANEOUS 0x0100u /* moves simultaneously in this turn */
#define GC_TURN_F_AUTOMOVES 0x0200u
#define GC_TURN_F_WAITING_FOR 0x0400u  /* the turn waits for this human */
typedef struct gcq_turn {
    int8_t player;
    int8_t team;
    int8_t slot_status;   /* SlotStatus of the core */
    int8_t blocker;       /* EndTurnBlockingTypes, -1 = none */
    uint16_t flags;       /* GC_TURN_F_* */
    int16_t turn_order;   /* hybrid turn: rank of the human among the humans who do not move simultaneously, by player id (0 = first); -1 = simultaneous turn */
    int32_t blocker_unit; /* unit id of the blocker's target, -1 = none */
    int32_t blocker_city; /* city id of the blocker's target, -1 = none */
    int16_t blocker_x;    /* plot of the target, -1 = none */
    int16_t blocker_y;
    uint32_t nick;        /* string, GCQ_NO_STRING = none */
} gcq_turn;               /* 24 bytes */

/* Q_DIPLO: args = {player}. One record per other living major civ, by id. war_locked_turns = turns before peace is possible again (0 = free). */
#define GC_DIPLO_F_MET 0x0001u
#define GC_DIPLO_F_AT_WAR 0x0002u
#define GC_DIPLO_F_HUMAN 0x0004u
#define GC_DIPLO_F_CAN_DECLARE_WAR 0x0008u
#define GC_DIPLO_F_CAN_MAKE_PEACE 0x0010u
typedef struct gcq_diplo {
    int8_t other;
    int8_t team;
    uint16_t flags;           /* GC_DIPLO_F_* */
    int16_t war_locked_turns;
    int16_t reserved;
} gcq_diplo;                  /* 8 bytes */

/* Q_REACHABLE_TREE: args = {player, unit id [, horizon 1..3]}. The plots the unit reaches within `horizon` turns, each with the plot it comes
 * from (the root, the unit's plot, is not listed; its children have the unit's plot as parent). Same pathfinder and costs as UNIT_MOVE_TO.
 * turn 0 = this turn, moves_left in 1/60 of a move on arrival. flags = GC_REACH_F_* (UNREACHABLE is not used). */
typedef struct gcq_tree_node {
    int16_t x;
    int16_t y;
    int16_t parent_x;
    int16_t parent_y;
    int16_t moves_left;
    uint8_t turn;
    uint8_t reserved0;
    uint16_t flags;
    uint16_t reserved1;
} gcq_tree_node;              /* 16 bytes */

/* Q_PLOT_BASIS: args = {} (the whole map, by index) or {plot index, ...} (those plots). The static terrain of the map, revealed or not,
 * natural wonders included, with no fog and no viewer_team (NP-D19). index = y * map_width + x. feature = terrain features only (natural
 * wonder, marsh, flood plains, ice), -1 otherwise. rivers = the 6 edges as in GC_PLOT_F_RIVER_SHIFT (bit d = river towards DirectionType d,
 * 0..5); river_flow = CvPlot::getRiver*FlowDirection of the own E, SE, SW edges, -1 = none. No resources, owners, improvements, units, cities. */
#define GC_BASIS_F_LAKE 0x0001u
#define GC_BASIS_F_COASTAL 0x0002u
#define GC_BASIS_F_NATURAL_WONDER 0x0004u
typedef struct gcq_plot_basis {
    uint32_t index;
    int8_t plot_type;     /* PlotTypes */
    uint8_t continent_art;
    int16_t terrain;      /* TerrainTypes */
    int16_t feature;      /* -1 = none */
    uint16_t flags;       /* GC_BASIS_F_* */
    uint8_t rivers;       /* bit d = river on the edge towards DirectionType d */
    int8_t river_flow[3]; /* E, SE, SW */
} gcq_plot_basis;         /* 16 bytes */

/* Q_COMBAT_PREVIEW: args = {player, attacker, x, y}; attacker >= 0 = a unit id of the player, attacker < 0 = -(city id). Only for a plot the
 * player's team sees now. Zero records when the attacker cannot attack that plot, else one. Forces x100. No side effects. */
#define GC_COMBAT_F_KILLS 0x0001u    /* the attack kills the target */
#define GC_COMBAT_F_CAPTURES 0x0002u /* the attack captures the target city */
#define GC_COMBAT_F_CITY 0x0004u     /* the target is a city */
typedef struct gcq_combat_preview {
    int16_t damage_to_target;
    int16_t damage_to_attacker;
    int32_t attacker_strength;
    int32_t defender_strength;
    int16_t target_hp;
    int16_t target_max_hp;
    uint16_t flags;       /* GC_COMBAT_F_* */
    uint16_t reserved;
} gcq_combat_preview;     /* 20 bytes */

/* Q_PLOTS_DIRTY: args = {}. The plots whose Q_PLOTS / Q_PLOT_BASIS data may have changed for any watched team since the last call. The call
 * resets the set; the first call (and the first after gc_load_game) lists every plot. It changes only the ABI layer's snapshot, never the game. */
typedef struct gcq_plot_dirty {
    uint32_t index;
} gcq_plot_dirty;         /* 4 bytes */

/* ---- Lua bridge (host functions callable from game Lua; not built yet) ---- */
typedef int32_t (*gc_lua_fn)(void* user, struct lua_State* L);

/* ---- functions ---------------------------------------------------------------------------------- */

uint32_t gc_abi_version(void);

/* Boots the core: game database and localization from host->read_file, DLL context. Returns NULL on
 * failure (err: GC_ERR_INVALID_ARG for a NULL host or read_file, GC_ERR_ALREADY_EXISTS when an
 * instance exists, GC_ERR_INTERNAL for a boot failure with the failed step in the message).
 * A process creates at most one instance in its lifetime: after gc_destroy a new gc_create fails with
 * GC_ERR_INTERNAL (the core is a process singleton). err may be NULL. */
gc_instance* gc_create(const gc_host* host, gc_error* err);

/* Tears the instance down; NULL-safe. The instance pointer is invalid afterwards. */
void gc_destroy(gc_instance* inst);

/* Registers a host function as table.name in the game's Lua. NOT_IMPLEMENTED in this revision. */
int32_t gc_lua_register(gc_instance* inst, const char* table, const char* name, gc_lua_fn fn, void* user, gc_error* err);

/* A new game from the setup JSON (an unknown key or a wrong value type is
 * GC_ERR_INVALID_ARG and does not consume the one-game slot). Runs Lua boot, pregame, map generation
 * and game start; takes seconds. Once per instance: a later call returns GC_ERR_ALREADY_EXISTS (also
 * after a failed start, which returns GC_ERR_INTERNAL with the failed step in the message). */
int32_t gc_new_game(gc_instance* inst, const char* setup_json, gc_error* err);

/* A game from gc_save bytes, instead of gc_new_game, on a new instance. GC_ERR_ALREADY_EXISTS when the
 * instance already has a game. GC_ERR_BAD_DATA (message = the reason) for bytes that are not GCS1, a
 * format version this core does not read, a truncated or corrupt buffer (length / xxh64 checksum), a
 * missing section, or a save of another gamecore build (build id: DLL version, game data); BAD_DATA is
 * found before anything is loaded and does not consume the one-game slot. A failure while loading is
 * GC_ERR_INTERNAL (slot used). After the call the game is where gc_save left it: gc_pump goes on. */
int32_t gc_load_game(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err);

/* Runs at most max_slices (>= 1) deterministic slices of the turn loop and fills out. Call again while
 * out->status is GC_PUMP_AI_RUNNING. GC_ERR_NOT_STARTED before a game; GC_ERR_STUCK (out filled,
 * status GC_PUMP_STUCK) when a turn does not end. Emits GC_EVENT_TURN_STARTED per turn change and
 * GC_EVENT_GAME_OVER once. */
int32_t gc_pump(gc_instance* inst, uint32_t max_slices, gc_pump_result* out, gc_error* err);

/* Validates a human command against the game and queues it on the loopback (the next gc_pump delivers it and runs its
 * missions). Implemented: GC_CMD_UNIT_* 1..12, GC_CMD_CITY_SET_PRODUCTION / _PURCHASE / _RANGE_ATTACK, GC_CMD_DECLARE_WAR,
 * GC_CMD_END_TURN, GC_CMD_FORCE_END_TURN, GC_CMD_CANCEL_END_TURN, GC_CMD_END_TURN_TIMEOUT, GC_CMD_MAKE_PEACE (v2: cmd->player and the team are humans at war; it is concluded
 * at once as the peace treaty deal, forced peace for PEACE_TREATY_LENGTH turns; reasons: NOT_HUMAN, AI_RUNNING, BAD_TARGET for a team without a human, RULE when not at war / locked /
 * a mod forbids it). GC_ERR_REJECTED (message = the reason, nothing queued) when the game refuses: cmd->player is not the
 * human, it is not the human's turn (AI still moving, or the turn already ended), unit missing / not owned / busy, target off the
 * map or unreachable, the unit cannot do it here, END_TURN while a blocker holds the turn (the message names the blocker and
 * its target) or while a queued command / busy unit asks for a gc_pump first. GC_ERR_INVALID_ARG for a wrong argc or an unknown
 * kind. END_TURN / FORCE_END_TURN take effect at once (turn complete is sent); gc_pump then runs the AI turn. */
int32_t gc_apply_command(gc_instance* inst, const gc_command* cmd, gc_error* err);

/* Fills out with a GCQ1 reply for query kind GC_Q_*. Implemented: GC_Q_GAME, GC_Q_PLOTS, GC_Q_UNITS, GC_Q_PLAYERS,
 * GC_Q_UNIT_ACTIONS, GC_Q_PATH, GC_Q_REACHABLE, GC_Q_CATALOG, GC_Q_CITIES, GC_Q_TOP_BAR, GC_Q_CITY_BUILDS, GC_Q_RANGE_TARGETS, GC_Q_NOTIFICATIONS, GC_Q_TURN, GC_Q_DIPLO. A unit (or a city) that
 * does not exist for the player is GC_ERR_INVALID_ARG. A wrong argc, an out-of-range argument
 * (viewer_team outside -1..MAX_TEAMS-1, table outside 0..GC_CATALOG_TABLE_COUNT-1) or an unknown kind is
 * GC_ERR_INVALID_ARG. out is untouched on error. */
int32_t gc_query(gc_instance* inst, int32_t kind, const int32_t* args, uint32_t argc, gc_buf* out, gc_error* err);

/* The game as GCS1 bytes (owned by the host: gc_buf_release). Only at a save point: the last gc_pump
 * returned WAITING_HUMAN and the host has not given a command since (with autoplay_human: between the
 * gc_pump that reached the human and the next one). Otherwise GC_ERR_REJECTED with the reason. Does not
 * change the game. v2 (N3.13): the same safe point for a network game with any number of humans (Q_GAME.safe_point / unsafe_reason say why not); the save carries the
 * handshake, kicks and the humans' per-turn state, the nicks and slot statuses (pregame); the connection is not saved (see gc_slot_control). GCS1 format version 2. */
int32_t gc_save(gc_instance* inst, gc_buf* out, gc_error* err);

/* 64-bit xxh64 digest of what viewer_team sees (-1 = the whole truth): the replies of Q_GAME, Q_PLOTS, Q_UNITS, Q_CITIES and
 * Q_TOP_BAR in that order, each as u8 kind + u32 length + the GCQ1 bytes (the exact canonical stream is in gc_digest.h). Identical
 * on every target for identical state and play; stable between calls; differs between viewers that see different things.
 * v2 (package P5): the top bars of the humans of the viewer's team go into the stream (v1: the active player's); viewer_team -2 = the
 * simulation digest for tests (the whole truth plus the game and map RNG state and the fields the queries must not touch).
 * viewer_team -3 (B4 gc-replica) = the replica digest: the pregame, the simulation part of -2, the runtime section a save would carry and which slots are on
 * line (the rules read it), no ABI-layer input (no Q_GAME), so a server and its replicas compare it (gc_journal_take's CHECK records). */
int32_t gc_state_digest(gc_instance* inst, int32_t viewer_team, uint64_t* out, gc_error* err);

/* v2 addition (follow-up of N3.13, backward compatible; GC_ABI_VERSION stays 2): the build id of this core, the string every GCS1 save header carries as
 * "build_id" ("lekmod-portable/<tag with the FIX / HYG set from build.sh>/dll <version>/guid <guid>/data <xxh64 of the game data>"). out gets UTF-8 bytes without a
 * terminating NUL (len = their count), owned by the host: gc_buf_release. Valid after gc_create (before a game too). GC_ERR_INVALID_ARG for a NULL instance or out. */
int32_t gc_build_id(gc_instance* inst, gc_buf* out, gc_error* err);

/* ---- slots (v2, section 4.8) ---- */
#define GC_SLOT_CONNECT 1    /* the human is on line: comes back to its slot, or takes a living AI major slot (nick required) */
#define GC_SLOT_DISCONNECT 2 /* connection lost: the slot stays human, the turn waits for it */
#define GC_SLOT_KICK 3       /* the room host kicked the player: the AI plays the slot */

/* Changes who plays a slot. Only at a safe point (the message says why not; GC_ERR_REJECTED with GC_REASON_NOT_SAFE_POINT), and only for a
 * living major civ that is not the server seat (GC_ERR_REJECTED with GC_REASON_SLOT_STATE otherwise; the same for DISCONNECT and KICK of a slot that is not a
 * human's). nick is UTF-8, up to 64 bytes, required for CONNECT onto an AI slot (GC_ERR_INVALID_ARG without it), optional for the other cases (NULL = keep).
 * Emits GC_EVENT_SLOT_CHANGED at once (args: player, slot_status, connected, kicked; text: the nick). GC_ERR_INVALID_ARG for a bad player or op. GC_ERR_NOT_STARTED
 * before a game.
 *  - DISCONNECT: the slot stays human and off line; the turn waits for it.
 *  - KICK: the AI plays the slot (SS_COMPUTER, the AI's handicap), also the rest of the turn it was kicked in; the next gc_pump runs it, so a second slot change or a
 *    gc_save must wait for that gc_pump (the host pumps to the human phase anyway).
 *  - CONNECT of the slot's own human: on line again. CONNECT onto an AI slot (a former human's or an original AI's): the nick, SS_TAKEN, the handicap of the humans of the
 *    game, no kick; its turn in the running game is not active (the AI has moved), so its end of turn is marked (PLAYER_TURN_ENDED) and the human plays from the next turn.
 *  - Connection is not restored: after gc_load_game every human is off line until the host's CONNECT (the turn does not wait differently for it); only a
 *    replica (gc_set_role) takes the slots on line from the save. */
int32_t gc_slot_control(gc_instance* inst, int32_t player, int32_t op, const char* nick, gc_error* err);


/* ---- replica (B4 gc-replica; append only, GC_ABI_VERSION stays 2) -------------------------------------------
 * The server runs the game and journals every input (accepted commands of every human, net messages of the humans' UIs, slot changes, the
 * slices of every gc_pump). A replica is a read-only copy: it loads a snapshot (gc_load_game, later gc_resync) and replays the journal with
 * gc_journal_apply; its UI's net messages (the DLL's gDLL->send*) go to an outbox the host sends to the server (gc_net_take -> gc_net_submit). */
#define GC_ROLE_PLAIN 0   /* default: the host drives the game, nothing is journaled */
#define GC_ROLE_SERVER 1  /* every input is journaled for gc_journal_take */
#define GC_ROLE_REPLICA 2 /* gc_pump, gc_apply_command, gc_slot_control and gc_net_submit refused; inputs come from gc_journal_apply; the test bots of the
                           * save never act; net messages queued outside the core's own work (the UI's) go to the outbox */

/* Sets the role of the instance. player: GC_ROLE_REPLICA = the seat whose UI the replica serves (-1 = none, a spectator); else -1.
 * GC_ROLE_REPLICA needs a game (gc_load_game first); it takes the slots on line from the loaded save (a server's gc_save keeps them in its "abi"
 * section; a plain load leaves the humans off line). GC_ERR_INVALID_ARG for an unknown role or a bad player. */
int32_t gc_set_role(gc_instance* inst, int32_t role, int32_t player, gc_error* err);

/* GCJ1 (little endian): "GCJ1", u16 format 1, u16 0, u32 count, then per record: u8 type (GC_JOURNAL_*), u8 player (0xFF = none),
 * u16 payload length, payload. A reader skips nothing: an unknown type is GC_ERR_BAD_DATA. */
#define GC_JOURNAL_COMMAND 1 /* payload: u16 kind, u16 argc, i32 args[argc], u16 text length (0xFFFF = NULL text), text; player = gc_command.player */
#define GC_JOURNAL_NET 2     /* payload: a GCN1 message body (i32 id, i32 args[9], u16 text length, text); player = the sender */
#define GC_JOURNAL_PUMP 3    /* payload: u32 slices, i32 status (GC_PUMP_* of the core, before the test bots' AI_RUNNING), i32 game turn after the call */
#define GC_JOURNAL_SLOT 4    /* payload: i32 op (GC_SLOT_*), u16 nick length (0xFFFF = NULL), nick; player = the slot */
#define GC_JOURNAL_CHECK 5   /* payload: u64 gc_state_digest(-3) at this point of the journal */
#define GC_JOURNAL_TAKE_CHECK 1u /* gc_journal_take: end a non-empty batch with a CHECK record (one serialization of the game, see the spec for its cost) */

/* GC_ROLE_SERVER: the inputs since the last call as GCJ1 bytes (owned by the host: gc_buf_release); the journal is empty afterwards.
 * GC_ERR_REJECTED in another role. */
int32_t gc_journal_take(gc_instance* inst, uint32_t flags, gc_buf* out, gc_error* err);

/* GC_ROLE_REPLICA: replays GCJ1 bytes in order. COMMAND runs the command as gc_apply_command would (it must be accepted), NET queues the message,
 * PUMP runs exactly that many slices (the result must be the server's), SLOT changes the slot, CHECK compares gc_state_digest(-3).
 * GC_ERR_DIVERGED (message: the record and what differs) when the replica left the server's game: the records before it are applied and the
 * replica must be resynced (gc_resync). GC_ERR_BAD_DATA for malformed bytes (nothing applied). Events are emitted as by gc_pump. */
int32_t gc_journal_apply(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err);

/* GCN1 (little endian): "GCN1", u16 format 1, u16 0, u32 count, then per message: u16 length, i32 id, i32 args[9], u16 text length, text.
 * id is the DLL net message (gDLL->send*): the RtNetId list of headless/portable/runtime/rt_net.h, append only; the arguments are those of the
 * send* call in that order (bools as 0 / 1). The sender is not in the message. */
#define GC_NET_ARGS 9

/* GC_ROLE_REPLICA: the net messages the UI sent since the last call (GCN1, owned by the host). Any role may call it (an empty list). */
int32_t gc_net_take(gc_instance* inst, gc_buf* out, gc_error* err);

/* Server: queues a human UI's net messages (GCN1) as sent by player, after checks: every id is one a UI sends (not the turn handshake:
 * the end of turn is GC_CMD_END_TURN), the player is a seated human whose turn is active and not ended (GC_REASON_* as gc_apply_command).
 * All or nothing. The DLL's handler runs them on the next gc_pump, each as that player; GC_ROLE_SERVER journals them. GC_ERR_REJECTED on a
 * replica. */
int32_t gc_net_submit(gc_instance* inst, int32_t player, const uint8_t* p, uint32_t n, gc_error* err);

/* Replaces the running game with GCS1 bytes in place (the exe's MP resync): the game objects are read again, the Lua state and contexts restart,
 * the loopback and outbox are emptied, the ABI layer takes the save's state (a replica keeps its role). GC_ERR_BAD_DATA as gc_load_game (nothing
 * changed); GC_ERR_INTERNAL when the load fails half way (the instance is unusable). */
int32_t gc_resync(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err);

/* ---- UI bridge (B4 M2 WO-1; append only, GC_ABI_VERSION stays 2) -------------------------------------
 * A replica (GC_ROLE_REPLICA) serves the original Lua UI. The UI's Lua state gets the DLL's own Lua libraries (gc_lua_attach_ui: Game, Players, Map, GameInfo,
 * DB.Query, ...); what the engine told the exe's UI (the DLL's gDLL->Gameplay* and DLLUI->* calls) comes out of gc_ui_sink as gc_ui_event, never as a pointer;
 * the part of the exe's interface state the DLL reads by pointer (selection, interface mode, city screen) lives in the core and is read and changed by gc_ui_op.
 * Everything here is for the replica role; a server (GC_ROLE_SERVER / PLAIN) behaves byte for byte as before and never calls the sink.
 * While the replica is at rest (between gc_* calls) the active player is its seat (gc_set_role's player), so Game.GetActivePlayer() is the UI's player; every
 * gc_* entry that runs rules (journal apply, resync, queries, digest) switches it to the server seat for the call, as on the server. */
#define GC_UI_MAX_ARGS 24
#define GC_UI_F_REBUILD 0x0001u /* produced by gc_ui_setup_graphical: the engine re-announces everything alive (reconciliation) */

typedef struct gc_ui_event {
    int32_t kind;            /* GC_UI_EV_* */
    uint32_t flags;          /* GC_UI_F_* */
    uint32_t n_args;
    int32_t args[GC_UI_MAX_ARGS];
    const char* text;        /* UTF-8 or NULL */
    const char* text2;       /* UTF-8 or NULL */
    const uint8_t* data;     /* GAME_EVENT / LUA_EVENT: the argument values (GC_UI_VAL_*); UNIT_MOVED: u32 plot indices; TRADE_NEW_ROUTE (more than 9 plots): int32 x, y pairs; else NULL */
    uint32_t data_len;
} gc_ui_event;               /* every pointer is valid during the callback only */

typedef struct gc_ui_sink {
    void* user;
    void (*event)(void* user, const gc_ui_event* ev); /* must not throw and must not call gc_* */
} gc_ui_sink;

/* Argument blocks used below. UNIT (14 ints): owner, id, x, y, unit type, culture (the civilization's art style), civilization, primary colour, secondary colour,
 * flag icon index (UnitInfo.UnitFlagIconOffset), fog state of its plot for the seat's team (FogOfWarModeTypes), selected (0/1), military (combat unit, 0/1),
 * visible (not invisible to the seat's team, 0/1). CITY (11 ints; text = the city's name key): owner, id, x, y, population, damage, max hit points, city size
 * type, team, puppet (0/1), capital (0/1). X/Y are plot coordinates (-1, -1 = no plot). */
#define GC_UI_UNIT_INTS 14
#define GC_UI_CITY_INTS 11

/* Kinds, engine -> UI (the gDLL->Gameplay* family) */
#define GC_UI_EV_UNIT_CREATED 1             /* UNIT */
#define GC_UI_EV_UNIT_MOVED 2               /* UNIT; data = u32 plot indices of the path (LE) */
#define GC_UI_EV_UNIT_TELEPORTED 3          /* UNIT, x, y (the plot it was put on) */
#define GC_UI_EV_UNIT_DESTROYED 4           /* UNIT */
#define GC_UI_EV_UNIT_DESTROYED_IN_COMBAT 5 /* UNIT */
#define GC_UI_EV_UNIT_SET_DAMAGE 6          /* UNIT, damage, previous damage */
#define GC_UI_EV_UNIT_FORTIFY 7             /* UNIT, fortify 0/1 */
#define GC_UI_EV_UNIT_WORK 8                /* UNIT, work type */
#define GC_UI_EV_UNIT_PARADROP 9            /* UNIT */
#define GC_UI_EV_UNIT_ACTIVATE 10           /* UNIT */
#define GC_UI_EV_UNIT_EMBARK 11             /* UNIT, embark 0/1 */
#define GC_UI_EV_UNIT_GARRISON 12           /* UNIT, garrison 0/1 */
#define GC_UI_EV_UNIT_SHOULD_DIM_FLAG 13    /* UNIT, dim 0/1 */
#define GC_UI_EV_UNIT_MARK_THREATENING 14   /* UNIT, mark 0/1 */
#define GC_UI_EV_UNIT_VISIBILITY 15         /* UNIT, visible 0/1, change invisibility 0/1, blend time in ms */
#define GC_UI_EV_UNIT_COMBAT 16             /* attacker side (kind 0 none / 1 unit / 2 city, owner, id), defender side (3 ints), interceptor side (3 ints), x, y, flags (1 attacker advances,
                                             * 2 visualize, 4 defender retaliates, 8 ranged), nuclear damage level */
#define GC_UI_EV_UNIT_MISSION_END 17        /* UNIT */
#define GC_UI_EV_UNIT_REBASED 18            /* UNIT, old x, old y, new x, new y */
#define GC_UI_EV_UNIT_RESET_ANIMATION 19    /* UNIT */
#define GC_UI_EV_CITY_CREATED 20            /* CITY, era */
#define GC_UI_EV_CITY_DESTROYED 21          /* CITY, new owner */
#define GC_UI_EV_CITY_CAPTURED 22           /* CITY, new owner */
#define GC_UI_EV_CITY_SET_DAMAGE 23         /* CITY, damage, previous damage */
#define GC_UI_EV_CITY_POPULATION_CHANGED 24 /* CITY, new population */
#define GC_UI_EV_CITY_COMBAT 25             /* as UNIT_COMBAT */
#define GC_UI_EV_ERA_CHANGED 30             /* player, era */
#define GC_UI_EV_BRIDGE_CHANGED 31          /* bridge 0/1, style */
#define GC_UI_EV_TECH_ACQUIRED 32           /* team, tech */
#define GC_UI_EV_FEATURE_CHANGED 33         /* x, y, feature */
#define GC_UI_EV_PLOT_STATE_CHANGE 34       /* x, y, revealed resource, revealed improvement, improvement state, route, route state */
#define GC_UI_EV_PLOT_ICON_STATE_CHANGE 35  /* x, y, revealed resource, revealed improvement, improvement state */
#define GC_UI_EV_NATURAL_WONDER_REVEALED 36 /* x, y */
#define GC_UI_EV_WALL_CREATED 37            /* x, y */
#define GC_UI_EV_DO_FX 38                   /* x, y (-1, -1 = none), fx type */
#define GC_UI_EV_FOW_CHANGED 39             /* x, y, FogOfWarModeTypes for the seat's team, set all 0/1 */
#define GC_UI_EV_YIELD_MIGHT_HAVE_CHANGED 40 /* x, y */
#define GC_UI_EV_WAR_STATE_CHANGED 41       /* team 1, team 2, at war 0/1 */
#define GC_UI_EV_MET_TEAM 42                /* my team, their team */
#define GC_UI_EV_DIPLO_AI_LEADER_MESSAGE 43 /* player, DiploUIStateTypes, LeaderheadAnimationTypes, data1; text = the leader's message */
#define GC_UI_EV_CLEAR_DIPLO_TRADE_TABLE 44 /* - */
#define GC_UI_EV_NOTIFY_AI_LEADERS_IN_GAME 45 /* - */
#define GC_UI_EV_NOTIFY_SPECIFIC_AI_LEADER 46 /* player */
#define GC_UI_EV_WONDER_CREATED 50          /* player, x, y, building, state */
#define GC_UI_EV_WONDER_EDITED 51           /* player, x, y, building, state */
#define GC_UI_EV_WONDER_REMOVED 52          /* player, x, y, building */
#define GC_UI_EV_SPACESHIP_CREATED 53       /* x, y, state */
#define GC_UI_EV_SPACESHIP_EDITED 54        /* x, y, state */
#define GC_UI_EV_SPACESHIP_REMOVED 55       /* x, y */
#define GC_UI_EV_OPEN_OPTIONS_SCREEN 56     /* - */
#define GC_UI_EV_SEARCH_FOR_PEDIA_ENTRY 57  /* text = the entry */
#define GC_UI_EV_OPEN_INFO_CORNER 58        /* info corner id */
#define GC_UI_EV_WORLD_ANCHOR 59            /* GenericWorldAnchorTypes, add 0/1, x, y, data1 */
#define GC_UI_EV_MINIMAP_UNIT_SELECT 60     /* x, y */
#define GC_UI_EV_MINIMAP_NOTIFICATION 61    /* x, y, id */
#define GC_UI_EV_GOLDEN_AGE_STARTED 62      /* - */
#define GC_UI_EV_GOLDEN_AGE_ENDED 63        /* - */
#define GC_UI_EV_TURN_CHANGED 64            /* new turn */
#define GC_UI_EV_ACTIVE_PLAYER_CHANGED 65   /* player */
#define GC_UI_EV_PUBLISH_ACTIVE_PLAYER 66   /* player, old player */
#define GC_UI_EV_PUBLISH_NEW_GAME_TURN 67   /* turn */
#define GC_UI_EV_PUBLISH_ERA_CHANGES 68     /* - */
#define GC_UI_EV_TRADE_NEW_ROUTE 69         /* route, player, connection type, plot count n, then x, y of the first (n <= 9) plots; data: int32 LE x, y of ALL plots when there are more than 9 */
#define GC_UI_EV_TRADE_ROUTE_DIRECTION 70   /* route, forwards 0/1 */
#define GC_UI_EV_TRADE_DESTROY_ROUTE 71     /* route, player */
#define GC_UI_EV_TRADE_ACTIVATE_POPUP_ROUTE 72 /* route */
#define GC_UI_EV_TRADE_DEACTIVATE_POPUP_ROUTE 73 /* - */
/* Kinds, engine -> UI (the DLLUI->* family) */
#define GC_UI_EV_ACTIVE_PLAYER_TURN_START 80 /* - (the seat's turn started) */
#define GC_UI_EV_ACTIVE_PLAYER_TURN_END 81  /* - */
#define GC_UI_EV_REMOTE_PLAYER_TURN_START 82 /* - */
#define GC_UI_EV_REMOTE_PLAYER_TURN_END 83  /* - */
#define GC_UI_EV_PLAYER_TURN_STATUS 84      /* TURN_STATUS_TYPE, player; text = tag */
#define GC_UI_EV_NOTIFICATION_ADDED 85      /* id, type, x, y, data1, data2, player; text = message, text2 = summary (the seat's notifications, found by comparison after each batch) */
#define GC_UI_EV_NOTIFICATION_REMOVED 86    /* id, player */
#define GC_UI_EV_NOTIFICATION_ACTIVATED 87  /* id, type, x, y, game index, extra game data, player; text = message (from ActivateNotification) */
#define GC_UI_EV_MESSAGE 88                 /* player, force 0/1, display length, InterfaceMessageTypes, x, y, source (0 plain, 1 unit, 2 plot, 3 city), owner, id; text = the line, text2 = sound */
#define GC_UI_EV_CLEAR_EVENT_MESSAGES 89    /* - */
#define GC_UI_EV_POPUP 90                   /* ButtonPopupTypes, data1, data2, data3, option1 0/1, option2 0/1, player; text = szText */
#define GC_UI_EV_POPUP_ARRAY 91             /* ButtonPopupTypes, n ints (<= 10), the ints, n bools (<= 10), the bools */
#define GC_UI_EV_POPUP_TEXT 92              /* x, y, delay in ms; text */
#define GC_UI_EV_SET_DIRTY 93               /* InterfaceDirtyBits, value 0/1 */
#define GC_UI_EV_LOOK_AT 94                 /* x, y, CameraLookAtTypes */
#define GC_UI_EV_CENTER_CAMERA 95           /* UNIT */
#define GC_UI_EV_SPECIFIC_CITY_INFO_DIRTY 96 /* CITY, CityUpdateTypes */
#define GC_UI_EV_UPDATE_END_TURN 97         /* - */
#define GC_UI_EV_END_TURN_BLOCKING_CHANGED 98 /* previous EndTurnBlockingTypes, new */
#define GC_UI_EV_PUBLISH_END_TURN_DIRTY 99  /* - */
#define GC_UI_EV_UPDATE_CITY_SCREEN 100     /* old city owner, old city id (-1 = none) */
#define GC_UI_EV_UNIT_SELECTION_CHANGED 101 /* selected count, head owner, head id (-1, -1 = none) */
#define GC_UI_EV_CITY_SELECTION_CHANGED 102 /* selected count, head owner, head id */
#define GC_UI_EV_INTERFACE_MODE_CHANGED 103 /* old mode, new mode, mode value */
#define GC_UI_EV_CITY_SCREEN_CHANGED 104    /* city screen up 0/1 (SerialEventEnterCityScreen / ExitCityScreen) */
#define GC_UI_EV_UPDATE_COUNTRY_BORDER 105  /* x, y */
#define GC_UI_EV_UPDATE_ALL_COUNTRY_BORDERS 106 /* - */
#define GC_UI_EV_ADD_HEX_TO_UI_RANGE 107    /* x, y */
#define GC_UI_EV_OPEN_END_GAME_MENU 108     /* - */
#define GC_UI_EV_OPEN_TECH_TREE 109         /* - */
#define GC_UI_EV_OPEN_PLAYER_DEAL_SCREEN 110 /* player */
#define GC_UI_EV_ADD_DEFERRED_WONDER_COMMAND 111 /* WonderCommandType, CITY, building, state */
#define GC_UI_EV_TOGGLE_STRATEGIC_VIEW 112  /* - */
#define GC_UI_EV_SET_TEMP_STRING 113        /* text */
#define GC_UI_EV_LOOK_AT_SELECTION_PLOT 114 /* release 0/1 */
#define GC_UI_EV_LOOK_AT_CITY_OFFSET 115    /* city id */
#define GC_UI_EV_FLASHING 116               /* player, flashing 0/1 */
#define GC_UI_EV_REFRESH_YIELD_VISIBLE_MODE 117 /* - */
/* Kinds, Lua -> UI */
#define GC_UI_EV_GAME_EVENT 150             /* text = GameEvents name, data = the arguments (GC_UI_VAL_*); after the core's listeners ran, only for names given to gc_ui_subscribe_game_event */
#define GC_UI_EV_LUA_EVENT 151              /* text = Events name, data = the arguments; a call of Events.<name>(...) from the core's Lua (Lekmod's contexts) */

/* Argument values of GAME_EVENT / LUA_EVENT (data): u8 count, then per value a u8 tag and its payload (little endian): NIL no payload; BOOL u8; NUM f64; STR u16 length + bytes;
 * VEC u8 count + f64 per element (a flat table of numbers, e.g. a hex position). Other Lua values are passed as NIL. */
#define GC_UI_VAL_NIL 0
#define GC_UI_VAL_BOOL 1
#define GC_UI_VAL_NUM 2
#define GC_UI_VAL_STR 3
#define GC_UI_VAL_VEC 4

/* Operations on the exe's interface state (gc_ui_op). in/out are int32 lists; "unit" and "city" are (owner, id) pairs, (-1, -1) = none. Selection changes and mode
 * changes also come out of the sink (UNIT_SELECTION_CHANGED, CITY_SELECTION_CHANGED, INTERFACE_MODE_CHANGED, CITY_SCREEN_CHANGED). */
#define GC_UI_OP_SELECT_UNIT 1              /* in: owner, id [, clear 0/1 (default 1), toggle 0/1 (default 0)] */
#define GC_UI_OP_SELECT_GROUP 2             /* in: owner, id [, shift, ctrl, alt]; the unit's group (ctrl: every unit of its type on the plot, alt: every unit of the owner on the plot) */
#define GC_UI_OP_HEAD_SELECTED_UNIT 3       /* out: owner, id */
#define GC_UI_OP_SELECTED_UNITS 4           /* out: owner, id, owner, id, ... (as many as fit; the first out value is not a count) */
#define GC_UI_OP_CLEAR_SELECTION 5          /* - */
#define GC_UI_OP_LAST_SELECTED_UNIT 6       /* out: owner, id */
#define GC_UI_OP_SELECT_CITY 7              /* in: owner, id [, test production 0/1] */
#define GC_UI_OP_HEAD_SELECTED_CITY 8       /* out: owner, id */
#define GC_UI_OP_CLEAR_SELECTED_CITIES 9    /* - */
#define GC_UI_OP_DO_SELECT_CITY_AT_PLOT 10  /* in: x, y; out: selected 0/1 (a city of the seat's player on that plot is selected, as UI.DoSelectCityAtPlot) */
#define GC_UI_OP_SET_INTERFACE_MODE 11      /* in: InterfaceModeTypes */
#define GC_UI_OP_GET_INTERFACE_MODE 12      /* out: mode */
#define GC_UI_OP_SET_INTERFACE_MODE_VALUE 13 /* in: value */
#define GC_UI_OP_GET_INTERFACE_MODE_VALUE 14 /* out: value */
#define GC_UI_OP_CAN_DO_INTERFACE_MODE 15   /* in: mode [, test visible 0/1]; out: 0/1 */
#define GC_UI_OP_SET_CITY_SCREEN_UP 16      /* in: 0/1 */
#define GC_UI_OP_IS_CITY_SCREEN_UP 17       /* out: 0/1 */
#define GC_UI_OP_CAN_SELECTION_LIST_FOUND 18 /* out: 0/1 */
#define GC_UI_OP_CAN_SELECTION_LIST_WORK 19 /* out: 0/1 */
#define GC_UI_OP_CAN_END_TURN 20            /* out: 0/1 (the host's canEndTurn of the seat) */
#define GC_UI_OP_ACTIVATE_NOTIFICATION 21   /* in: lookup id of a notification of the seat: the DLL's CvNotifications::Activate (popup, look at, selection) comes out of the sink */
#define GC_UI_OP_REMOVE_NOTIFICATION 22     /* in: lookup id: NOTIFICATION_REMOVED for the UI; the game state (dismissed flag) is the server's and does not change */
#define GC_UI_OP_GET_CYCLE_SELECTION_COUNTER 23 /* out: counter */
#define GC_UI_OP_SET_CYCLE_SELECTION_COUNTER 24 /* in: counter */
#define GC_UI_OP_IS_UNIT_SELECTED 25        /* in: owner, id; out: 0/1 */
#define GC_UI_OP_IS_CITY_SELECTED 26        /* in: owner, id; out: 0/1 */
#define GC_UI_OP_SELECT_ALL_AT_PLOT 27      /* in: x, y; every unit of the seat's player on the plot */
#define GC_UI_OP_GET_SELECTION_COUNT 28     /* out: number of selected units */
#define GC_UI_OP_COUNT 29

/* Puts the DLL's Lua libraries into the UI's Lua state L: Game, Map, Players, Teams, the enums and GameInfoTypes, the types Unit / City / Plot / ..., GameInfo and
 * DB.Query over this instance's database (a real SQLite: PRAGMA table_info works), GameDefines and PreGame (reads of this instance's pregame). Not set: include, print, Locale,
 * Events, UI, Network, the sandbox (the host's). The stack of L is the same afterwards, also on an error. The instance must outlive L (GameInfo holds its database).
 * GC_ERR_NOT_STARTED before a game; GC_ERR_INTERNAL when a registration raised a Lua error (the message says which). Any role. */
int32_t gc_lua_attach_ui(gc_instance* inst, struct lua_State* L, gc_error* err);

/* After gc_resync: the units, cities, plots, areas, deals and the league of the old game are gone. Clears the cache of each of these types in L
 * (<Type>.__instances) and the "__instance" of every table in it, so a method call on an old object is a Lua error ("Not a valid instance" / "Instance does not exist"),
 * not a read of freed memory. Players, Teams and TeamTechs live in static arrays and keep working (the same objects before and after a resync). NULL L: nothing. */
int32_t gc_lua_invalidate_ui(gc_instance* inst, struct lua_State* L, gc_error* err);

/* The event sink (see above); NULL clears it. The struct is copied. Events are delivered only while the role is GC_ROLE_REPLICA. */
int32_t gc_set_ui_sink(gc_instance* inst, const gc_ui_sink* sink, gc_error* err);

/* Reads or changes the exe's interface state: op = GC_UI_OP_*. in / n_in as listed above; out has room for *n_out values and *n_out is set to the number written
 * (NULL out / n_out = the result is not wanted). GC_ERR_INVALID_ARG for an unknown op or too few inputs, GC_ERR_NOT_FOUND for a unit, city or notification that does not
 * exist, GC_ERR_REJECTED outside GC_ROLE_REPLICA. */
int32_t gc_ui_op(gc_instance* inst, int32_t op, const int32_t* in, uint32_t n_in, int32_t* out, uint32_t* n_out, gc_error* err);

/* The UI listens to GameEvents.<name>: after the core's listeners ran, the same call with the same arguments comes out of the sink as GAME_EVENT (the UI's results do not
 * count; a server has no UI listeners). Names accumulate until the instance dies. */
int32_t gc_ui_subscribe_game_event(gc_instance* inst, const char* name, gc_error* err);

/* After gc_load_game or gc_resync: CvMap::setupGraphical and CvPlayer::setupGraphical of every living player, as the exe does after a load. Events with GC_UI_F_REBUILD.
 * GC_ERR_REJECTED outside GC_ROLE_REPLICA. */
int32_t gc_ui_setup_graphical(gc_instance* inst, gc_error* err);

#ifdef __cplusplus
}
#endif

#endif /* GAMECORE_ABI_H */
