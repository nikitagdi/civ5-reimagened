/* C ABI of the `loc` crate (Firaxis localization grammar). Link the static library built with
 *   cargo rustc -p loc --features capi --crate-type staticlib --release
 * All strings are UTF-8 and NUL-terminated. No function throws or unwinds.
 */
#ifndef CIV5_LOC_H
#define CIV5_LOC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct LocHandle LocHandle;

/* kind: 0 int (int_val), 1 int64 (int_val), 2 float (num_val), 3 double (num_val), 4 string (str_val) */
typedef struct LocArg {
    int kind;
    int64_t int_val;
    double num_val;
    const char *str_val;
} LocArg;

/* gender and plurality may be NULL. Pointers stay valid until the callback is called again. */
typedef struct LocRow {
    const char *text;
    const char *gender;
    const char *plurality;
} LocRow;

/* Returns non-zero when the row exists. */
typedef int (*LocLookupFn)(void *user, const char *tag, LocRow *out);

/* plural_rule is the Languages.PluralRule column, 1..16 (en_US: 2). */
LocHandle *loc_new(const char *language_type, const char *display_name, uint32_t plural_rule);
void loc_free(LocHandle *h);

/* Rows not added with loc_add_text are fetched through f (may be NULL). */
void loc_set_lookup(LocHandle *h, LocLookupFn f, void *user);
/* Returns 1 on success. Replaces an existing row of the same tag. */
int loc_add_text(LocHandle *h, const char *tag, const char *text, const char *gender, const char *plurality);

/* Localization::String::Compose: template + arguments -> text. snprintf convention: the output is
 * truncated to cap-1 bytes and terminated; the return value is the length of the full result. An invalid
 * template gives "". */
size_t loc_compose(LocHandle *h, const char *tmpl, const LocArg *args, size_t nargs, char *out, size_t cap);
/* Locale.ConvertTextKey: a text key (a missing key is its own text) + arguments -> text. */
size_t loc_lookup(LocHandle *h, const char *key, const LocArg *args, size_t nargs, char *out, size_t cap);
int loc_has_key(LocHandle *h, const char *key);

/* which: 0 = missing keys, 1 = bad templates */
uint64_t loc_stat(LocHandle *h, int which);
/* Pops the oldest warning; returns its length or SIZE_MAX when there is none. */
size_t loc_next_warning(LocHandle *h, char *out, size_t cap);

#ifdef __cplusplus
}
#endif

#endif
