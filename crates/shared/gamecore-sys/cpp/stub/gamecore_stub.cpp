// Fake gamecore for gamecore_abi.h v2, used where the real archive is not linked (Windows, CI).
// Deterministic and clock-free: create, new_game (setup JSON checked like the real core), gc_pump
// (two slices to WAITING_HUMAN on turn 1), gc_state_digest, gc_save / gc_load_game, buffers, errors
// and events. gc_apply_command, gc_query and gc_lua_register validate their arguments and return
// GC_ERR_NOT_IMPLEMENTED.
// v2: the stub plays the B1 game (one human) only. game_type "single" is accepted; a setup that uses
// another game_type or a v2 key (players, turn_mode, turn_timer, autoplay_humans, world_builder_*)
// and gc_slot_control answer
// GC_ERR_NOT_STARTED; the real core takes them.
// gc_save writes a GCS1 buffer laid out like the real core's (32-byte fixed header with the xxh64 of the
// body, then sections "header" and "stub"), only at the save point (the last pump ended at
// WAITING_HUMAN); gc_load_game checks it the way the real core does (BAD_DATA before anything is
// loaded, build id "gamecore-stub/1").
// Differences from the real core: gc_create is allowed again after gc_destroy (the real core is a
// process singleton); the digest is FNV-1a over (seed, slices run, turn), not xxh64 of the queries.
#include "gamecore_abi.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <string>
#include <vector>

extern "C" {
#include <lua.h>
}

namespace {

enum {
    SLICES_TO_HUMAN = 2,  // slices from game start until the human is up
    HUMAN_PLAYER = 0,
    BLOCKER_UNITS = 0  // fake EndTurnBlockingTypes value while the human is up
};

bool g_instance_exists = false;

}  // namespace

struct gc_instance {
    gc_host host;
    bool started;
    uint64_t seed;
    int32_t turn;
    int32_t slices_run;  // slices of the current turn already done
    int32_t role;        // B4 gc-replica: GC_ROLE_*
    gc_ui_sink ui_sink;  // B4 M2 WO-1: stored, never called (the stub has no game to tell the UI about)
};

namespace {

void set_err(gc_error* err, int32_t code, const std::string& msg) {
    if (!err) return;
    err->code = code;
    err->reason = GC_REASON_NONE;
    std::strncpy(err->message, msg.c_str(), sizeof(err->message) - 1);
    err->message[sizeof(err->message) - 1] = 0;
}

int32_t fail(gc_error* err, int32_t code, const std::string& msg) {
    set_err(err, code, msg);
    return code;
}

void clear_err(gc_error* err) {
    if (!err) return;
    err->code = GC_OK;
    err->message[0] = 0;
    err->reason = GC_REASON_NONE;
}

void host_log(gc_instance* g, int32_t level, const std::string& msg) {
    if (g->host.log) g->host.log(g->host.user, level, msg.c_str());
}

void host_event(gc_instance* g, int32_t kind, int32_t a0, uint32_t argc) {
    if (!g->host.game_event) return;
    int32_t args[1] = {a0};
    gc_event ev;
    ev.kind = kind;
    ev.argc = argc;
    ev.args = args;
    ev.text = 0;
    ev.text2 = 0;
    g->host.game_event(g->host.user, &ev);
}

// ---- setup JSON: a small validating parser; keeps top-level keys and their value kinds ----------

enum ValueKind { V_NUMBER, V_STRING, V_OBJECT, V_OTHER };

struct Member {
    std::string key;
    ValueKind kind;
    double number;
    std::string text;  // the value of a string member
};

struct Json {
    const char* p;
    std::string error;

    void ws() {
        while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') ++p;
    }
    bool fail_at(const char* what) {
        if (error.empty()) error = what;
        return false;
    }
    bool string(std::string* out) {
        if (*p != '"') return fail_at("expected a string");
        ++p;
        std::string s;
        while (*p && *p != '"') {
            if (static_cast<unsigned char>(*p) < 0x20) return fail_at("control character in a string");
            if (*p == '\\') {
                ++p;
                if (!*p) break;
                if (*p == 'u') {
                    for (int i = 0; i < 4; ++i) {
                        ++p;
                        if (!*p || !std::strchr("0123456789abcdefABCDEF", *p)) return fail_at("bad \\u escape");
                    }
                    s += '?';
                } else if (std::strchr("\"\\/bfnrt", *p)) {
                    s += *p;
                } else {
                    return fail_at("bad escape");
                }
            } else {
                s += *p;
            }
            ++p;
        }
        if (*p != '"') return fail_at("unterminated string");
        ++p;
        if (out) *out = s;
        return true;
    }
    bool number(double* out) {
        char* end = 0;
        double v = std::strtod(p, &end);
        if (end == p) return fail_at("expected a value");
        p = end;
        if (out) *out = v;
        return true;
    }
    bool literal(const char* word) {
        size_t n = std::strlen(word);
        if (std::strncmp(p, word, n) != 0) return fail_at("expected a value");
        p += n;
        return true;
    }
    bool value(ValueKind* kind, double* num, int depth, std::string* text = 0) {
        if (depth > 16) return fail_at("nested too deep");
        ws();
        *num = 0;
        if (*p == '"') {
            *kind = V_STRING;
            return string(text);
        }
        if (*p == '{') {
            *kind = V_OBJECT;
            return object(0, depth + 1);
        }
        if (*p == '[') {
            *kind = V_OTHER;
            ++p;
            ws();
            if (*p == ']') {
                ++p;
                return true;
            }
            for (;;) {
                ValueKind k;
                double d;
                if (!value(&k, &d, depth + 1)) return false;
                ws();
                if (*p == ',') {
                    ++p;
                    continue;
                }
                if (*p == ']') {
                    ++p;
                    return true;
                }
                return fail_at("expected , or ]");
            }
        }
        if (*p == 't') {
            *kind = V_OTHER;
            return literal("true");
        }
        if (*p == 'f') {
            *kind = V_OTHER;
            return literal("false");
        }
        if (*p == 'n') {
            *kind = V_OTHER;
            return literal("null");
        }
        *kind = V_NUMBER;
        return number(num);
    }
    // members is filled for the top-level object only.
    bool object(std::vector<Member>* members, int depth) {
        ws();
        if (*p != '{') return fail_at("setup must be a JSON object");
        ++p;
        ws();
        if (*p == '}') {
            ++p;
            return true;
        }
        for (;;) {
            ws();
            Member m;
            if (!string(&m.key)) return false;
            ws();
            if (*p != ':') return fail_at("expected :");
            ++p;
            if (!value(&m.kind, &m.number, depth, &m.text)) return false;
            if (members) members->push_back(m);
            ws();
            if (*p == ',') {
                ++p;
                continue;
            }
            if (*p == '}') {
                ++p;
                return true;
            }
            return fail_at("expected , or }");
        }
    }
};

struct SetupKey {
    const char* name;
    ValueKind kind;
};

const SetupKey SETUP_KEYS[] = {
    {"seed", V_NUMBER},         {"map_seed", V_NUMBER},    {"human", V_OBJECT},
    {"ai_count", V_NUMBER},     {"world_size", V_STRING},  {"map_script", V_STRING},
    {"map_options", V_OBJECT},  {"handicap", V_STRING},    {"game_speed", V_STRING},
    {"start_era", V_STRING},    {"minor_civs", V_STRING},  {"game_options", V_OBJECT},
    {"auto_choices", V_OBJECT}, {"diplo_auto_decline", V_NUMBER},
    {"autoplay_human", V_NUMBER}, {"observer", V_NUMBER}};

// Keys of the v2 setup that only the real v2 core takes.
const char* const V2_SETUP_KEYS[] = {"game_type",      "players",
                                     "turn_mode",      "turn_timer",
                                     "autoplay_humans", "world_builder_map",
                                     "world_builder_scenario"};

// Returns GC_OK and fills *seed, GC_ERR_INVALID_ARG with the reason in *why, or GC_ERR_NOT_STARTED
// for a v2 key (a well-formed v2 setup this stub cannot play).
int32_t parse_setup(const char* text, uint64_t* seed, std::string* why) {
    Json j;
    j.p = text;
    std::vector<Member> members;
    bool ok = j.object(&members, 0);
    if (ok) {
        j.ws();
        if (*j.p) ok = j.fail_at("trailing characters after the setup object");
    }
    if (!ok) {
        *why = "setup JSON: " + j.error;
        return GC_ERR_INVALID_ARG;
    }
    *seed = 0;
    for (size_t i = 0; i < members.size(); ++i) {
        // game_type "single" is the B1 game the stub plays; anything else of it is a v2 setup.
        if (members[i].key == "game_type" && members[i].kind == V_STRING && members[i].text == "single")
            continue;
        for (size_t k = 0; k < sizeof(V2_SETUP_KEYS) / sizeof(V2_SETUP_KEYS[0]); ++k) {
            if (members[i].key == V2_SETUP_KEYS[k]) {
                *why = "the stub does not implement the v2 setup (key '" + members[i].key + "')";
                return GC_ERR_NOT_STARTED;
            }
        }
        const SetupKey* known = 0;
        for (size_t k = 0; k < sizeof(SETUP_KEYS) / sizeof(SETUP_KEYS[0]); ++k)
            if (members[i].key == SETUP_KEYS[k].name) known = &SETUP_KEYS[k];
        if (!known) {
            *why = "setup JSON: unknown key '" + members[i].key + "'";
            return GC_ERR_INVALID_ARG;
        }
        if (members[i].kind != known->kind) {
            *why = "setup JSON: wrong value type for '" + members[i].key + "'";
            return GC_ERR_INVALID_ARG;
        }
        if (members[i].key == "seed") *seed = static_cast<uint64_t>(static_cast<long long>(members[i].number));
    }
    return GC_OK;
}

uint64_t fnv_step(uint64_t h, uint64_t v) {
    for (int i = 0; i < 8; ++i) {
        h ^= (v >> (8 * i)) & 0xFF;
        h *= 1099511628211ULL;
    }
    return h;
}

// ---- GCS1 (the real core's container: rt_save.h) ------------------------------------------------

const char STUB_BUILD_ID[] = "gamecore-stub/1";
const uint32_t GCS_FORMAT_VERSION = 1;
const uint32_t GCS_FIXED_HEADER = 32;

const uint64_t P1 = 11400714785074694791ULL;
const uint64_t P2 = 14029467366897019727ULL;
const uint64_t P3 = 1609587929392839161ULL;
const uint64_t P4 = 9650029242287828579ULL;
const uint64_t P5 = 2870177450012600261ULL;

uint64_t rotl(uint64_t x, int r) {
    return (x << r) | (x >> (64 - r));
}
uint64_t rd64(const uint8_t* p) {
    uint64_t v = 0;
    for (int i = 7; i >= 0; --i) v = (v << 8) | p[i];
    return v;
}
uint32_t rd32(const uint8_t* p) {
    return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) | (static_cast<uint32_t>(p[2]) << 16) |
           (static_cast<uint32_t>(p[3]) << 24);
}
uint64_t round64(uint64_t acc, uint64_t lane) {
    acc += lane * P2;
    acc = rotl(acc, 31);
    return acc * P1;
}
uint64_t merge64(uint64_t acc, uint64_t v) {
    acc ^= round64(0, v);
    return acc * P1 + P4;
}

// xxh64 (seed 0 here), as the real core's checksum.
uint64_t xxh64(const uint8_t* p, size_t len, uint64_t seed) {
    const uint8_t* end = p + len;
    uint64_t h;
    if (len >= 32) {
        uint64_t v1 = seed + P1 + P2, v2 = seed + P2, v3 = seed, v4 = seed - P1;
        while (p + 32 <= end) {
            v1 = round64(v1, rd64(p));
            v2 = round64(v2, rd64(p + 8));
            v3 = round64(v3, rd64(p + 16));
            v4 = round64(v4, rd64(p + 24));
            p += 32;
        }
        h = rotl(v1, 1) + rotl(v2, 7) + rotl(v3, 12) + rotl(v4, 18);
        h = merge64(h, v1);
        h = merge64(h, v2);
        h = merge64(h, v3);
        h = merge64(h, v4);
    } else {
        h = seed + P5;
    }
    h += static_cast<uint64_t>(len);
    while (p + 8 <= end) {
        h ^= round64(0, rd64(p));
        h = rotl(h, 27) * P1 + P4;
        p += 8;
    }
    if (p + 4 <= end) {
        h ^= static_cast<uint64_t>(rd32(p)) * P1;
        h = rotl(h, 23) * P2 + P3;
        p += 4;
    }
    while (p < end) {
        h ^= static_cast<uint64_t>(*p) * P5;
        h = rotl(h, 11) * P1;
        ++p;
    }
    h ^= h >> 33;
    h *= P2;
    h ^= h >> 29;
    h *= P3;
    h ^= h >> 32;
    return h;
}

void put_le(std::vector<uint8_t>& out, uint64_t v, int bytes) {
    for (int i = 0; i < bytes; ++i) out.push_back(static_cast<uint8_t>((v >> (8 * i)) & 0xFF));
}

void put_section(std::vector<uint8_t>& body, const std::string& name, const std::vector<uint8_t>& bytes) {
    put_le(body, name.size(), 2);
    body.insert(body.end(), name.begin(), name.end());
    put_le(body, bytes.size(), 4);
    body.insert(body.end(), bytes.begin(), bytes.end());
}

struct Section {
    std::string name;
    const uint8_t* data;
    uint32_t size;
};

// The sections of a GCS1 buffer, or false with the reason.
bool parse_gcs1(const uint8_t* p, uint32_t n, std::vector<Section>* out, std::string* why) {
    if (n < 4 || std::memcmp(p, "GCS1", 4) != 0) {
        *why = "not a GCS1 save (bad magic)";
        return false;
    }
    if (n < GCS_FIXED_HEADER) {
        *why = "truncated GCS1 header";
        return false;
    }
    if (rd32(p + 4) != GCS_FORMAT_VERSION) {
        *why = "GCS1 format version this core does not read";
        return false;
    }
    if (rd32(p + 8) != GCS_FIXED_HEADER) {
        *why = "bad GCS1 header size";
        return false;
    }
    const uint32_t count = rd32(p + 12);
    const uint64_t body_len = rd64(p + 16);
    if (body_len != static_cast<uint64_t>(n - GCS_FIXED_HEADER)) {
        *why = "truncated or padded save (body length does not match)";
        return false;
    }
    const uint8_t* body = p + GCS_FIXED_HEADER;
    if (xxh64(body, static_cast<size_t>(body_len), 0) != rd64(p + 24)) {
        *why = "corrupt save (xxh64 checksum does not match)";
        return false;
    }
    const uint8_t* q = body;
    const uint8_t* end = body + body_len;
    for (uint32_t i = 0; i < count; ++i) {
        if (end - q < 2) break;
        const uint32_t name_len = static_cast<uint32_t>(q[0]) | (static_cast<uint32_t>(q[1]) << 8);
        q += 2;
        if (static_cast<uint32_t>(end - q) < name_len + 4) break;
        Section s;
        s.name.assign(reinterpret_cast<const char*>(q), name_len);
        q += name_len;
        s.size = rd32(q);
        q += 4;
        if (static_cast<uint32_t>(end - q) < s.size) break;
        s.data = q;
        q += s.size;
        out->push_back(s);
    }
    if (out->size() != count || q != end) {
        *why = "malformed GCS1 section table";
        return false;
    }
    return true;
}

void release_vector(gc_buf* buf) {
    delete static_cast<std::vector<uint8_t>*>(buf->owner);
}

const Section* find_section(const std::vector<Section>& sections, const char* name) {
    for (size_t i = 0; i < sections.size(); ++i)
        if (sections[i].name == name) return &sections[i];
    return 0;
}

}  // namespace

#define GC_GUARD_BEGIN try {
#define GC_GUARD_END(err)                                    \
    }                                                        \
    catch (const std::bad_alloc&) {                          \
        return fail(err, GC_ERR_INTERNAL, "out of memory");  \
    }                                                        \
    catch (...) {                                            \
        return fail(err, GC_ERR_INTERNAL, "exception");      \
    }

extern "C" uint32_t gc_abi_version(void) {
    return GC_ABI_VERSION;
}

extern "C" void gc_buf_release(gc_buf* buf) {
    if (!buf) return;
    if (buf->release) buf->release(buf);
    buf->data = 0;
    buf->len = 0;
    buf->release = 0;
    buf->owner = 0;
}

extern "C" gc_instance* gc_create(const gc_host* host, gc_error* err) {
    clear_err(err);
    if (!host || !host->read_file) {
        fail(err, GC_ERR_INVALID_ARG, "gc_create: host and host->read_file must not be NULL");
        return 0;
    }
    if (g_instance_exists) {
        fail(err, GC_ERR_ALREADY_EXISTS, "gc_create: an instance exists");
        return 0;
    }
    try {
        gc_instance* g = new gc_instance();
        g->host = *host;
        g->started = false;
        g->seed = 0;
        g->turn = 0;
        g->slices_run = 0;
        g_instance_exists = true;
        host_log(g, GC_LOG_INFO, "gamecore stub v2: instance created");
        return g;
    } catch (...) {
        fail(err, GC_ERR_INTERNAL, "gc_create: out of memory");
        return 0;
    }
}

extern "C" void gc_destroy(gc_instance* inst) {
    if (!inst) return;
    delete inst;
    g_instance_exists = false;
}

extern "C" int32_t gc_lua_register(gc_instance* inst, const char* table, const char* name, gc_lua_fn fn,
                                   void* user, gc_error* err) {
    (void)user;
    clear_err(err);
    if (!inst || !table || !name || !fn) return fail(err, GC_ERR_INVALID_ARG, "gc_lua_register: NULL argument");
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_lua_register is not implemented in the stub");
}

extern "C" int32_t gc_new_game(gc_instance* inst, const char* setup_json, gc_error* err) {
    clear_err(err);
    if (!inst || !setup_json) return fail(err, GC_ERR_INVALID_ARG, "gc_new_game: NULL argument");
    GC_GUARD_BEGIN
    uint64_t seed = 0;
    std::string why;
    // Validation first: a bad setup does not use up the one-game slot.
    const int32_t parsed = parse_setup(setup_json, &seed, &why);
    if (parsed != GC_OK) return fail(err, parsed, why);
    if (inst->started) return fail(err, GC_ERR_ALREADY_EXISTS, "a game was already started in this instance");
    inst->started = true;
    inst->seed = seed;
    inst->turn = 0;
    inst->slices_run = 0;
    // Exercises the VFS callback.
    gc_buf buf = {0, 0, 0, 0};
    if (inst->host.read_file(inst->host.user, "vfs-index.json", &buf) == GC_OK) {
        char msg[96];
        std::sprintf(msg, "gamecore stub v2: vfs-index.json has %u bytes", static_cast<unsigned>(buf.len));
        host_log(inst, GC_LOG_DEBUG, msg);
        gc_buf_release(&buf);
    }
    host_log(inst, GC_LOG_INFO, std::string("gamecore stub v2: new game, setup ") + setup_json);
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_load_game(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err) {
    clear_err(err);
    if (!inst || !p || n == 0) return fail(err, GC_ERR_INVALID_ARG, "gc_load_game: NULL or empty save");
    if (inst->started) return fail(err, GC_ERR_ALREADY_EXISTS, "a game was already started in this instance");
    GC_GUARD_BEGIN
    // Everything is checked before the instance changes: BAD_DATA leaves the one-game slot free.
    std::vector<Section> sections;
    std::string why;
    if (!parse_gcs1(p, n, &sections, &why)) return fail(err, GC_ERR_BAD_DATA, why);
    const Section* header = find_section(sections, "header");
    const Section* state = find_section(sections, "stub");
    if (!header || !state || state->size != 16) return fail(err, GC_ERR_BAD_DATA, "missing or malformed section");
    const std::string text(reinterpret_cast<const char*>(header->data), header->size);
    const std::string key = std::string("\nbuild_id=") + STUB_BUILD_ID + "\n";
    if (text.find(key) == std::string::npos)
        return fail(err, GC_ERR_BAD_DATA, "save of another gamecore build (this core is " + std::string(STUB_BUILD_ID) + ")");
    inst->started = true;
    inst->seed = rd64(state->data);
    inst->turn = static_cast<int32_t>(rd32(state->data + 8));
    inst->slices_run = static_cast<int32_t>(rd32(state->data + 12));
    host_log(inst, GC_LOG_INFO, "gamecore stub v2: game loaded");
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_pump(gc_instance* inst, uint32_t max_slices, gc_pump_result* out, gc_error* err) {
    clear_err(err);
    if (!inst || !out || max_slices == 0) return fail(err, GC_ERR_INVALID_ARG, "gc_pump: NULL out or max_slices == 0");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_new_game not done)");
    GC_GUARD_BEGIN
    int32_t used = 0;
    // Turn 0 is the setup; the first pump slices run turn 1 up to the human.
    while (inst->slices_run < SLICES_TO_HUMAN && static_cast<uint32_t>(used) < max_slices) {
        if (inst->slices_run == 0) {
            inst->turn += 1;
            host_event(inst, GC_EVENT_TURN_STARTED, inst->turn, 1);
        }
        ++inst->slices_run;
        ++used;
        if (inst->slices_run == SLICES_TO_HUMAN) host_event(inst, GC_EVENT_PLAYER_TURN_STARTED, HUMAN_PLAYER, 1);
    }
    out->turn = inst->turn;
    out->slices_used = used;
    if (inst->slices_run < SLICES_TO_HUMAN) {
        out->status = GC_PUMP_AI_RUNNING;
        out->active_player = 1;
        out->blocker = -1;
    } else {
        out->status = GC_PUMP_WAITING_HUMAN;
        out->active_player = HUMAN_PLAYER;
        out->blocker = BLOCKER_UNITS;
    }
    out->humans_left = out->status == GC_PUMP_WAITING_HUMAN ? 1 : 0;
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_apply_command(gc_instance* inst, const gc_command* cmd, gc_error* err) {
    clear_err(err);
    if (!inst || !cmd || (cmd->argc > 0 && !cmd->args)) return fail(err, GC_ERR_INVALID_ARG, "gc_apply_command: bad argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_new_game not done)");
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_apply_command is not implemented in the stub");
}

extern "C" int32_t gc_query(gc_instance* inst, int32_t kind, const int32_t* args, uint32_t argc, gc_buf* out,
                            gc_error* err) {
    clear_err(err);
    if (!inst || !out || (argc > 0 && !args) || kind < GC_Q_GAME || kind > GC_Q_PLOTS_DIRTY)
        return fail(err, GC_ERR_INVALID_ARG, "gc_query: bad argument or unknown query kind");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_new_game not done)");
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_query is not implemented in the stub");
}

extern "C" int32_t gc_save(gc_instance* inst, gc_buf* out, gc_error* err) {
    clear_err(err);
    if (!inst || !out) return fail(err, GC_ERR_INVALID_ARG, "gc_save: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_new_game not done)");
    if (inst->slices_run < SLICES_TO_HUMAN)
        return fail(err, GC_ERR_REJECTED, "not at a save point: the last gc_pump did not end at WAITING_HUMAN");
    GC_GUARD_BEGIN
    char header[160];
    std::sprintf(header, "format=GCS1 %u\nbuild_id=%s\nabi=%u\nturn=%d\n", static_cast<unsigned>(GCS_FORMAT_VERSION),
                 STUB_BUILD_ID, static_cast<unsigned>(GC_ABI_VERSION), static_cast<int>(inst->turn));
    std::vector<uint8_t> state;
    put_le(state, inst->seed, 8);
    put_le(state, static_cast<uint32_t>(inst->turn), 4);
    put_le(state, static_cast<uint32_t>(inst->slices_run), 4);
    std::vector<uint8_t> body;
    put_section(body, "header", std::vector<uint8_t>(header, header + std::strlen(header)));
    put_section(body, "stub", state);
    std::vector<uint8_t>* bytes = new std::vector<uint8_t>();
    bytes->reserve(GCS_FIXED_HEADER + body.size());
    static const char magic[] = "GCS1";
    bytes->insert(bytes->end(), magic, magic + 4);
    put_le(*bytes, GCS_FORMAT_VERSION, 4);
    put_le(*bytes, GCS_FIXED_HEADER, 4);
    put_le(*bytes, 2, 4);
    put_le(*bytes, body.size(), 8);
    put_le(*bytes, xxh64(&body[0], body.size(), 0), 8);
    bytes->insert(bytes->end(), body.begin(), body.end());
    out->data = &(*bytes)[0];
    out->len = bytes->size();
    out->release = release_vector;
    out->owner = bytes;
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_build_id(gc_instance* inst, gc_buf* out, gc_error* err) {
    clear_err(err);
    if (!inst || !out) return fail(err, GC_ERR_INVALID_ARG, "gc_build_id: NULL argument");
    GC_GUARD_BEGIN
    std::vector<uint8_t>* bytes = new std::vector<uint8_t>(STUB_BUILD_ID, STUB_BUILD_ID + std::strlen(STUB_BUILD_ID));
    out->data = &(*bytes)[0];
    out->len = bytes->size();
    out->release = release_vector;
    out->owner = bytes;
    return GC_OK;
    GC_GUARD_END(err)
}

// v2: arguments are checked, then NOT_STARTED (the stub has no slots; the real core does from package P8).
extern "C" int32_t gc_slot_control(gc_instance* inst, int32_t player, int32_t op, const char* nick, gc_error* err) {
    clear_err(err);
    if (!inst) return fail(err, GC_ERR_INVALID_ARG, "gc_slot_control: NULL instance");
    if (player < 0 || player >= 22) return fail(err, GC_ERR_INVALID_ARG, "gc_slot_control: player is not a major civ slot");
    if (op != GC_SLOT_CONNECT && op != GC_SLOT_DISCONNECT && op != GC_SLOT_KICK)
        return fail(err, GC_ERR_INVALID_ARG, "gc_slot_control: unknown op");
    if (nick && std::strlen(nick) > 64) return fail(err, GC_ERR_INVALID_ARG, "gc_slot_control: nick longer than 64 bytes");
    return fail(err, GC_ERR_NOT_STARTED, "gc_slot_control is not implemented in the stub");
}

extern "C" int32_t gc_state_digest(gc_instance* inst, int32_t viewer_team, uint64_t* out, gc_error* err) {
    clear_err(err);
    if (!inst || !out || viewer_team < -1) return fail(err, GC_ERR_INVALID_ARG, "gc_state_digest: bad argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_new_game not done)");
    uint64_t h = 14695981039346656037ULL;
    h = fnv_step(h, inst->seed);
    h = fnv_step(h, static_cast<uint64_t>(inst->turn));
    h = fnv_step(h, static_cast<uint64_t>(inst->slices_run));
    *out = h;
    return GC_OK;
}

// ---- B4 gc-replica: roles are kept and checked; the journal and the net outbox are always empty (the stub has no inputs to record);
// replaying a non-empty journal, submitting messages and resyncing answer NOT_IMPLEMENTED (the real core does them).
namespace {

void hand_out_list(gc_buf* out, const char magic[4]) {
    std::vector<uint8_t>* bytes = new std::vector<uint8_t>(magic, magic + 4);
    const uint8_t rest[8] = {1, 0, 0, 0, 0, 0, 0, 0};  // u16 format 1, u16 0, u32 count 0
    bytes->insert(bytes->end(), rest, rest + 8);
    out->data = &(*bytes)[0];
    out->len = bytes->size();
    out->release = release_vector;
    out->owner = bytes;
}

}  // namespace

extern "C" int32_t gc_set_role(gc_instance* inst, int32_t role, int32_t player, gc_error* err) {
    clear_err(err);
    if (!inst) return fail(err, GC_ERR_INVALID_ARG, "gc_set_role: NULL instance");
    if (role != GC_ROLE_PLAIN && role != GC_ROLE_SERVER && role != GC_ROLE_REPLICA) return fail(err, GC_ERR_INVALID_ARG, "gc_set_role: unknown role");
    if (player < -1 || player >= 22) return fail(err, GC_ERR_INVALID_ARG, "gc_set_role: bad player");
    if (role == GC_ROLE_REPLICA && !inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet (gc_load_game not done)");
    inst->role = role;
    return GC_OK;
}

extern "C" int32_t gc_journal_take(gc_instance* inst, uint32_t flags, gc_buf* out, gc_error* err) {
    clear_err(err);
    (void)flags;
    if (!inst || !out) return fail(err, GC_ERR_INVALID_ARG, "gc_journal_take: NULL argument");
    if (inst->role != GC_ROLE_SERVER) return fail(err, GC_ERR_REJECTED, "gc_journal_take: only a server keeps a journal");
    GC_GUARD_BEGIN
    hand_out_list(out, "GCJ1");
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_journal_apply(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err) {
    clear_err(err);
    if (!inst || (n > 0 && !p)) return fail(err, GC_ERR_INVALID_ARG, "gc_journal_apply: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    if (inst->role != GC_ROLE_REPLICA) return fail(err, GC_ERR_REJECTED, "gc_journal_apply: only a replica replays a journal");
    if (n < 12 || std::memcmp(p, "GCJ1", 4) != 0) return fail(err, GC_ERR_BAD_DATA, "gc_journal_apply: not GCJ1 data");
    const uint32_t count = (uint32_t)p[8] | ((uint32_t)p[9] << 8) | ((uint32_t)p[10] << 16) | ((uint32_t)p[11] << 24);
    if (count == 0 && n == 12) return GC_OK;
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_journal_apply: the stub has no game to replay into");
}

extern "C" int32_t gc_net_take(gc_instance* inst, gc_buf* out, gc_error* err) {
    clear_err(err);
    if (!inst || !out) return fail(err, GC_ERR_INVALID_ARG, "gc_net_take: NULL argument");
    GC_GUARD_BEGIN
    hand_out_list(out, "GCN1");
    return GC_OK;
    GC_GUARD_END(err)
}

extern "C" int32_t gc_net_submit(gc_instance* inst, int32_t player, const uint8_t* p, uint32_t n, gc_error* err) {
    clear_err(err);
    (void)player;
    if (!inst || (n > 0 && !p)) return fail(err, GC_ERR_INVALID_ARG, "gc_net_submit: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    if (inst->role == GC_ROLE_REPLICA) return fail(err, GC_ERR_REJECTED, "gc_net_submit: a replica sends to the server");
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_net_submit is not implemented in the stub");
}

extern "C" int32_t gc_resync(gc_instance* inst, const uint8_t* p, uint32_t n, gc_error* err) {
    clear_err(err);
    if (!inst || (n > 0 && !p)) return fail(err, GC_ERR_INVALID_ARG, "gc_resync: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_resync is not implemented in the stub");
}

// ---- B4 M2 WO-1: the UI bridge. The stub has no game: attach puts empty Game / Players / Map / GameInfo / DB tables, the rest is empty or not implemented. ----

extern "C" int32_t gc_lua_attach_ui(gc_instance* inst, lua_State* L, gc_error* err) {
    clear_err(err);
    if (!inst || !L) return fail(err, GC_ERR_INVALID_ARG, "gc_lua_attach_ui: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    static const char* const names[] = {"Game", "Players", "Teams", "Map", "GameInfo", "DB", "GameDefines", "PreGame"};
    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); ++i) {
        lua_getglobal(L, names[i]);
        const bool present = !lua_isnil(L, -1);
        lua_pop(L, 1);
        if (present) continue;
        lua_newtable(L);
        lua_setglobal(L, names[i]);
    }
    return GC_OK;
}

extern "C" int32_t gc_lua_invalidate_ui(gc_instance* inst, lua_State* L, gc_error* err) {
    clear_err(err);
    if (!inst) return fail(err, GC_ERR_INVALID_ARG, "gc_lua_invalidate_ui: NULL instance");
    (void)L;  // the stub creates no instances
    return GC_OK;
}

extern "C" int32_t gc_set_ui_sink(gc_instance* inst, const gc_ui_sink* sink, gc_error* err) {
    clear_err(err);
    if (!inst) return fail(err, GC_ERR_INVALID_ARG, "gc_set_ui_sink: NULL instance");
    if (sink) {
        inst->ui_sink = *sink;
    } else {
        inst->ui_sink.user = nullptr;
        inst->ui_sink.event = nullptr;
    }
    return GC_OK;
}

extern "C" int32_t gc_ui_op(gc_instance* inst, int32_t op, const int32_t* in, uint32_t n_in, int32_t* out, uint32_t* n_out, gc_error* err) {
    clear_err(err);
    if (!inst || (n_in > 0 && !in)) return fail(err, GC_ERR_INVALID_ARG, "gc_ui_op: NULL argument");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    if (inst->role != GC_ROLE_REPLICA) return fail(err, GC_ERR_REJECTED, "gc_ui_op: only a replica has a UI");
    if (op <= 0 || op >= GC_UI_OP_COUNT) return fail(err, GC_ERR_INVALID_ARG, "gc_ui_op: unknown op");
    (void)out;
    if (n_out) *n_out = 0;
    return fail(err, GC_ERR_NOT_IMPLEMENTED, "gc_ui_op is not implemented in the stub");
}

extern "C" int32_t gc_ui_subscribe_game_event(gc_instance* inst, const char* name, gc_error* err) {
    clear_err(err);
    if (!inst || !name || !name[0]) return fail(err, GC_ERR_INVALID_ARG, "gc_ui_subscribe_game_event: NULL argument");
    return GC_OK;
}

extern "C" int32_t gc_ui_setup_graphical(gc_instance* inst, gc_error* err) {
    clear_err(err);
    if (!inst) return fail(err, GC_ERR_INVALID_ARG, "gc_ui_setup_graphical: NULL instance");
    if (!inst->started) return fail(err, GC_ERR_NOT_STARTED, "no game yet");
    if (inst->role != GC_ROLE_REPLICA) return fail(err, GC_ERR_REJECTED, "gc_ui_setup_graphical: only a replica has a UI");
    return GC_OK;  // no units, no cities: nothing to announce
}
