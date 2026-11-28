/* SPDX-License-Identifier: MIT */
/*
 * Sine, cosine and tangent of a float for pvgcube, which has no C runtime.
 * Worked in double and rounded once to float, so that for the angles the
 * cube uses they give what a correctly rounded library gives; the Mac's
 * does, and the reference is drawn with it. `CUBE_MATH_NAME(sin)` names each
 * function, so that a test on the Mac can build them beside the library's.
 *
 * The argument is brought into [-pi, pi] with 2 pi in three parts (Cody and
 * Waite), which keeps it far more exact than a float's step for the
 * angles the cube turns through, then the Taylor series is summed to its 27th power.
 */
#ifndef CUBE_MATH_H
#define CUBE_MATH_H

#ifndef CUBE_MATH_NAME
#define CUBE_MATH_NAME(name) name##f
#endif

/* 2 pi = A + B + C: A is the float nearest it, B and C what is left, as doubles. */
#define CUBE_TWO_PI_A 6.2831854820251465
#define CUBE_TWO_PI_B -1.748455600074497e-07
#define CUBE_TWO_PI_C -1.0687562935444062e-23
#define CUBE_INV_TWO_PI 0.15915494309189535

static double cube_reduce(double x)
{
	double k = x * CUBE_INV_TWO_PI;
	long long n = (long long)(k < 0 ? k - 0.5 : k + 0.5);

	k = (double)n;
	return ((x - k * CUBE_TWO_PI_A) - k * CUBE_TWO_PI_B) - k * CUBE_TWO_PI_C;
}

/* sin and cos of r in [-pi, pi] at once. */
static void cube_sin_cos(double r, double *s, double *c)
{
	double r2 = r * r, term_s = r, term_c = 1, sum_s = r, sum_c = 1;
	int i;

	for (i = 1; i <= 14; i++) {
		term_s *= -r2 / ((2.0 * i) * (2.0 * i + 1));
		term_c *= -r2 / ((2.0 * i - 1) * (2.0 * i));
		sum_s += term_s;
		sum_c += term_c;
	}
	*s = sum_s;
	*c = sum_c;
}

float CUBE_MATH_NAME(sin)(float x)
{
	double s, c;

	cube_sin_cos(cube_reduce((double)x), &s, &c);
	return (float)s;
}

float CUBE_MATH_NAME(cos)(float x)
{
	double s, c;

	cube_sin_cos(cube_reduce((double)x), &s, &c);
	return (float)c;
}

float CUBE_MATH_NAME(tan)(float x)
{
	double s, c;

	cube_sin_cos(cube_reduce((double)x), &s, &c);
	return (float)(s / c);
}

#endif
