/* SPDX-License-Identifier: MIT */
/*
 * The cube's scene, for every program that draws it: the guest's, the
 * reference on the Mac, and the rehearsal through the helper. Plain C with
 * nothing but <math.h> behind it, so that all three draw from the same
 * numbers.
 *
 * The shaders are tools/pvg-shader/shaders/cube.vert and cube.frag: a place,
 * a normal and a place in the picture for each corner, 32 bytes; a block of
 * constants with two matrices and a light; one picture.
 */
#ifndef PVG_CUBE_SCENE_H
#define PVG_CUBE_SCENE_H

#include <math.h>
#include <stdint.h>

#define CUBE_CORNERS 24
#define CUBE_CORNER_BYTES 32
#define CUBE_INDICES 36
#define CUBE_PICTURE_SIZE 64	/* a side, in pixels; four bytes a pixel */

/* What the shaders call Scene: matrices by columns, as Metal reads them. */
struct cube_constants {
	float clip_from_model[16];
	float world_from_model[16];
	float light[4];
};

/* Six faces of four corners: place, normal, place in the picture. */
static inline void cube_corners(float out[CUBE_CORNERS * 8])
{
	/* For each face its normal and the two directions that span it. */
	static const float faces[6][9] = {
		{ 0, 0, 1,   1, 0, 0,   0, 1, 0 },
		{ 0, 0, -1,  -1, 0, 0,  0, 1, 0 },
		{ 1, 0, 0,   0, 0, -1,  0, 1, 0 },
		{ -1, 0, 0,  0, 0, 1,   0, 1, 0 },
		{ 0, 1, 0,   1, 0, 0,   0, 0, -1 },
		{ 0, -1, 0,  1, 0, 0,   0, 0, 1 },
	};
	static const float corners[4][2] = { { -1, -1 }, { 1, -1 }, { 1, 1 }, { -1, 1 } };

	for (int face = 0; face < 6; face++) {
		const float *n = faces[face], *u = faces[face] + 3, *v = faces[face] + 6;

		for (int corner = 0; corner < 4; corner++) {
			float *c = out + (face * 4 + corner) * 8;
			float a = corners[corner][0], b = corners[corner][1];

			for (int i = 0; i < 3; i++) {
				c[i] = n[i] + a * u[i] + b * v[i];
				c[3 + i] = n[i];
			}
			/* The picture's first row is its top. */
			c[6] = (a + 1) / 2;
			c[7] = (1 - b) / 2;
		}
	}
}

/* Two triangles a face, counter-clockwise seen from outside. */
static inline void cube_indices(uint16_t out[CUBE_INDICES])
{
	static const uint16_t face[6] = { 0, 1, 2, 0, 2, 3 };

	for (int i = 0; i < CUBE_INDICES; i++)
		out[i] = (uint16_t)(i / 6 * 4 + face[i % 6]);
}

/*
 * The picture: eight by eight squares, each face of the cube showing all of
 * them. Light squares, and dark ones whose colour says where they are, so
 * that a face turned or mirrored is seen. A pixel is blue, green, red, alpha
 * in memory.
 */
static inline void cube_picture(uint32_t out[CUBE_PICTURE_SIZE * CUBE_PICTURE_SIZE])
{
	for (int y = 0; y < CUBE_PICTURE_SIZE; y++) {
		for (int x = 0; x < CUBE_PICTURE_SIZE; x++) {
			uint32_t column = (uint32_t)x / 8, row = (uint32_t)y / 8;
			uint32_t pixel = 0xfff0f0e0;

			if ((column + row) % 2)
				pixel = 0xff000040 | (40 + column * 30) << 16 | (40 + row * 30) << 8;
			out[y * CUBE_PICTURE_SIZE + x] = pixel;
		}
	}
}

/* out = a times b, both by columns. */
static inline void cube_multiply(float out[16], const float a[16], const float b[16])
{
	float result[16];

	for (int column = 0; column < 4; column++)
		for (int row = 0; row < 4; row++) {
			float sum = 0;

			for (int k = 0; k < 4; k++)
				sum += a[k * 4 + row] * b[column * 4 + k];
			result[column * 4 + row] = sum;
		}
	for (int i = 0; i < 16; i++)
		out[i] = result[i];
}

/*
 * The constants for a cube turned by `angle`, in radians, seen by a target
 * `aspect` times as wide as high. Metal's clip space: y points up, z runs
 * from 0 at the near plane to 1 at the far one.
 */
static inline void cube_constants_at(struct cube_constants *out, float angle, float aspect)
{
	float ca = cosf(angle), sa = sinf(angle);
	float cb = cosf(angle * 0.7f), sb = sinf(angle * 0.7f);
	/* A turn about y, then one about x. */
	const float about_y[16] = { ca, 0, -sa, 0,  0, 1, 0, 0,  sa, 0, ca, 0,  0, 0, 0, 1 };
	const float about_x[16] = { 1, 0, 0, 0,  0, cb, sb, 0,  0, -sb, cb, 0,  0, 0, 0, 1 };
	/* The eye is five units back. */
	const float view[16] = { 1, 0, 0, 0,  0, 1, 0, 0,  0, 0, 1, 0,  0, 0, -5, 1 };
	float f = 1.0f / tanf(0.4f), near = 0.5f, far = 20.0f;
	const float projection[16] = {
		f / aspect, 0, 0, 0,
		0, f, 0, 0,
		0, 0, far / (near - far), -1,
		0, 0, near * far / (near - far), 0,
	};
	float clip_from_world[16];

	cube_multiply(out->world_from_model, about_x, about_y);
	cube_multiply(clip_from_world, projection, view);
	cube_multiply(out->clip_from_model, clip_from_world, out->world_from_model);
	/* The light comes from the upper right, in front. A face turned away keeps a fifth. */
	out->light[0] = 0.48f;
	out->light[1] = 0.64f;
	out->light[2] = 0.6f;
	out->light[3] = 0.2f;
}

#endif
