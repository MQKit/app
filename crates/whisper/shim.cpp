// A few plain functions over whisper.cpp, so Rust needs no generated bindings (the big
// whisper_full_params struct stays on this side).
#include "whisper.h"
#include <cstdint>

extern "C" {

typedef void (*shim_progress_fn)(int percent, void * user);
typedef bool (*shim_abort_fn)(void * user);

struct shim_cb {
    shim_progress_fn progress;
    shim_abort_fn abort;
    void * user;
};

static void quiet(enum ggml_log_level, const char *, void *) {}

static void on_progress(struct whisper_context *, struct whisper_state *, int p, void * data) {
    auto * cb = (shim_cb *) data;
    if (cb->progress) cb->progress(p, cb->user);
}

static bool on_abort(void * data) {
    auto * cb = (shim_cb *) data;
    return cb->abort ? cb->abort(cb->user) : false;
}

// whisper.cpp writes its logs to stderr; the worker's stdout is JSON only either way,
// but the log is noise: silenced.
void shim_quiet(void) {
    whisper_log_set(quiet, nullptr);
}

struct whisper_context * shim_load(const char * model_path) {
    struct whisper_context_params cp = whisper_context_default_params();
    cp.use_gpu = false;
    return whisper_init_from_file_with_params(model_path, cp);
}

void shim_free(struct whisper_context * ctx) {
    whisper_free(ctx);
}

// 0 = done, 1 = stopped, other = failed.
int shim_run(struct whisper_context * ctx, const float * samples, int n, const char * language, bool translate,
             int threads, int max_len, int beam_size, const char * prompt,
             shim_progress_fn progress, shim_abort_fn abort, void * user) {
    struct whisper_full_params p = whisper_full_default_params(beam_size > 1 ? WHISPER_SAMPLING_BEAM_SEARCH : WHISPER_SAMPLING_GREEDY);
    p.n_threads = threads;
    p.translate = translate;
    p.language = language;
    p.detect_language = false;
    p.print_special = false;
    p.print_progress = false;
    p.print_realtime = false;
    p.print_timestamps = false;
    p.suppress_blank = true;
    if (beam_size > 1) p.beam_search.beam_size = beam_size;
    if (max_len > 0) {
        p.token_timestamps = true;
        p.max_len = max_len;
        p.split_on_word = true;
    }
    if (prompt && prompt[0]) p.initial_prompt = prompt;
    shim_cb cb = { progress, abort, user };
    p.progress_callback = on_progress;
    p.progress_callback_user_data = &cb;
    p.abort_callback = on_abort;
    p.abort_callback_user_data = &cb;
    int r = whisper_full(ctx, p, samples, n);
    if (r != 0) return abort && abort(user) ? 1 : 2;
    return 0;
}

int shim_n_segments(struct whisper_context * ctx) { return whisper_full_n_segments(ctx); }
const char * shim_segment_text(struct whisper_context * ctx, int i) { return whisper_full_get_segment_text(ctx, i); }
int64_t shim_segment_t0(struct whisper_context * ctx, int i) { return whisper_full_get_segment_t0(ctx, i); }
int64_t shim_segment_t1(struct whisper_context * ctx, int i) { return whisper_full_get_segment_t1(ctx, i); }
const char * shim_language(struct whisper_context * ctx) { return whisper_lang_str(whisper_full_lang_id(ctx)); }

}
