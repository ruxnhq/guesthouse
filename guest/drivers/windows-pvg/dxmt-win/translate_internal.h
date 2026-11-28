/* SPDX-License-Identifier: MIT */
/*
 * What translate.c and translate_blit.c share: the helpers that fail an
 * encoder, note a dropped state, look up an object, and name what a
 * command lists or writes. Not part of translate.h's interface.
 */
#ifndef PVG_TRANSLATE_INTERNAL_H
#define PVG_TRANSLATE_INTERNAL_H

#include "translate.h"

#define NEXT(cmd) ((const struct wmtcmd_base *)(cmd)->next.ptr)

void pvgx_fail(struct pvgx_encoder *encoder, const char *what, unsigned value);
uint32_t pvgx_listed(const struct pvgx_info *info);
uint32_t pvgx_texture_at(const struct pvgx_info *info, unsigned *level, unsigned *slice);
bool pvgx_level_missing(struct pvgx_encoder *encoder, const struct pvgx_info *info, unsigned level);
void pvgx_drop(struct pvgx_encoder *encoder, const char *what);
bool pvgx_look(struct pvgx_encoder *encoder, obj_handle_t handle, enum pvgx_kind kind, struct pvgx_info *info);
uint32_t pvgx_buffer_number(struct pvgx_encoder *encoder, obj_handle_t handle, uint64_t *base);

#endif
