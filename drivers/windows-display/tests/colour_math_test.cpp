#include "check.h"
#include "hlsl_shim.h"
#include "../GuesthouseDisplay/Guesthouse/Shaders/ColourMath.hlsli"
#include "../GuesthouseDisplay/Guesthouse/ColourTransform.h"

using namespace Guesthouse;

static const float NO_SDR_BOOST = 1.0f;

static float3 Through(const Rows& rows, float3 colour)
{
	const auto row = [&](int i) { return float4(rows.value[i][0], rows.value[i][1], rows.value[i][2], rows.value[i][3]); };
	return ApplyRows(row(0), row(1), row(2), colour);
}

// With no transform, SDR through the pass must come out as it went in, for every 8-bit value.
static void Sdr_is_bit_exact_through_the_whole_pipeline()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Sdr, GammaTransform{});
	for (uint value = 0; value < 256; ++value)
	{
		const float texel = value / 255.0f;
		float3 light = DecodeSource(SOURCE_SRGB, float3(texel, texel, texel), NO_SDR_BOOST);
		light = Through(rows, light);
		const uint packed = PackXrgb8888(EncodeWire(WIRE_SDR, light, NO_SDR_BOOST));
		CHECK(packed == (0xFF000000u | value << 16 | value << 8 | value));
	}
}

static void Pq_matches_its_published_points()
{
	CHECK_NEAR(NitsToPq1(0.0f), 0.0, 1e-6);
	CHECK_NEAR(NitsToPq1(100.0f), 0.5081, 0.0002);
	CHECK_NEAR(NitsToPq1(1000.0f), 0.7518, 0.0002);
	CHECK_NEAR(NitsToPq1(10000.0f), 1.0, 1e-5);
	CHECK_NEAR(PqToNits1(1.0f), 10000.0, 0.5);
}

static void Pq_round_trips()
{
	for (float nits : {0.1f, 1.0f, 80.0f, 203.0f, 1000.0f, 4000.0f})
	{
		CHECK_NEAR(PqToNits1(NitsToPq1(nits)), nits, nits * 0.001);
	}
}

// scRGB's 1.0 is 80 nits, so white at 12.5 is 1,000 nits on the wire.
static void Scrgb_white_lands_at_the_right_brightness()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const float3 light = Through(rows, DecodeSource(SOURCE_SCRGB, float3(12.5f, 12.5f, 12.5f), NO_SDR_BOOST));
	const float3 encoded = EncodeWire(WIRE_HDR, light, NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.7518, 0.0005);
	CHECK_NEAR(encoded.y, 0.7518, 0.0005);
	CHECK_NEAR(encoded.z, 0.7518, 0.0005);
}

// On an HDR desktop, SDR white is where the user's slider put it, not 80 nits.
static void Sdr_content_sits_at_the_sdr_white_level()
{
	const float sdrWhite = 240.0f / SCRGB_NITS;
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const float3 light = Through(rows, DecodeSource(SOURCE_SRGB, float3(1, 1, 1), sdrWhite));
	const float3 encoded = EncodeWire(WIRE_HDR, light, sdrWhite);
	CHECK_NEAR(PqToNits1(encoded.x), 240.0, 0.5);
}

static void Pq_input_passes_through_unchanged()
{
	const Rows rows = ComposeToWire(Primaries::Bt2020, WireFormat::Hdr, GammaTransform{});
	const float3 texel(0.3f, 0.5f, 0.7f);
	const float3 encoded = EncodeWire(WIRE_HDR, Through(rows, DecodeSource(SOURCE_PQ, texel, NO_SDR_BOOST)), NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.3, 0.0005);
	CHECK_NEAR(encoded.y, 0.5, 0.0005);
	CHECK_NEAR(encoded.z, 0.7, 0.0005);
}

static void Bt709_to_bt2020_is_the_published_matrix()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	CHECK_NEAR(rows.value[0][0], 0.6274, 0.0002);
	CHECK_NEAR(rows.value[0][1], 0.3293, 0.0002);
	CHECK_NEAR(rows.value[0][2], 0.0433, 0.0002);
	CHECK_NEAR(rows.value[1][1], 0.9195, 0.0002);
	CHECK_NEAR(rows.value[2][2], 0.8956, 0.0002);
}

static void An_identity_transform_changes_nothing()
{
	GammaTransform transform;
	transform.matrixEnabled = true;
	transform.matrix[0][0] = transform.matrix[1][1] = transform.matrix[2][2] = 1.0f;
	const Rows plain = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const Rows same = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, transform);
	for (int row = 0; row < 3; ++row)
	{
		for (int column = 0; column < 4; ++column)
		{
			CHECK_NEAR(same.value[row][column], plain.value[row][column], 1e-6);
		}
	}
}

static void The_scalar_multiplies_the_matrix_and_its_offset()
{
	GammaTransform transform;
	transform.matrixEnabled = true;
	transform.matrix[0][0] = transform.matrix[1][1] = transform.matrix[2][2] = 1.0f;
	transform.scalar = 0.5f;
	const Rows halved = ComposeToWire(Primaries::Bt709, WireFormat::Sdr, transform);
	const float3 white = Through(halved, float3(1, 1, 1));
	CHECK_NEAR(white.x, 0.5, 1e-5);
	CHECK_NEAR(white.y, 0.5, 1e-5);
	CHECK_NEAR(white.z, 0.5, 1e-5);

	// An offset of the white point's XYZ adds one white, in wire RGB, after the scalar has halved it.
	transform.matrix[0][3] = 0.95047f;
	transform.matrix[1][3] = 1.0f;
	transform.matrix[2][3] = 1.08883f;
	const float3 black = Through(ComposeToWire(Primaries::Bt709, WireFormat::Sdr, transform), float3(0, 0, 0));
	CHECK_NEAR(black.x, 0.5, 0.001);
	CHECK_NEAR(black.y, 0.5, 0.001);
	CHECK_NEAR(black.z, 0.5, 0.001);
}

static void Channels_land_where_drm_says()
{
	CHECK(PackXrgb8888(float3(1, 0, 0)) == 0xFFFF0000u);
	CHECK(PackXrgb8888(float3(0, 0, 1)) == 0xFF0000FFu);
	CHECK(PackXbgr2101010(float3(1, 0, 0)) == 0xC00003FFu);
	CHECK(PackXbgr2101010(float3(0, 1, 0)) == 0xC00FFC00u);
	CHECK(PackXbgr2101010(float3(0, 0, 1)) == 0xFFF00000u);
}

static void Out_of_range_never_wraps()
{
	CHECK(PackXrgb8888(float3(2.0f, -1.0f, 0)) == 0xFFFF0000u);
	const float3 encoded = EncodeWire(WIRE_HDR, float3(-1.0f, 1000.0f, 0), NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.0, 1e-6);
	CHECK_NEAR(encoded.y, 1.0, 1e-5);
}

static GammaTransform IdentityTable()
{
	GammaTransform transform;
	transform.lutEnabled = true;
	transform.lut.resize(GammaLutEntries);
	for (std::uint32_t entry = 0; entry < GammaLutEntries; ++entry)
	{
		const float value = static_cast<float>(entry) / (GammaLutEntries - 1);
		transform.lut[entry] = {value, value, value, 0.0f};
	}
	return transform;
}

static GammaTransform IdentityMatrix()
{
	GammaTransform transform;
	transform.matrixEnabled = true;
	transform.matrix[0][0] = transform.matrix[1][1] = transform.matrix[2][2] = 1.0f;
	return transform;
}

static void Only_a_transform_that_changes_nothing_is_skipped()
{
	CHECK(LeavesPixelsAlone(GammaTransform{}));
	CHECK(LeavesPixelsAlone(IdentityTable()));
	CHECK(LeavesPixelsAlone(IdentityMatrix()));

	GammaTransform darker = IdentityTable();
	darker.lut[2048][1] *= 0.99f;
	CHECK(!LeavesPixelsAlone(darker));

	GammaTransform brighter = IdentityMatrix();
	brighter.scalar = 1.1f;
	CHECK(!LeavesPixelsAlone(brighter));

	GammaTransform offset = IdentityMatrix();
	offset.matrix[1][3] = 0.01f;
	CHECK(!LeavesPixelsAlone(offset));

	GammaTransform shortTable;
	shortTable.lutEnabled = true;
	CHECK(!LeavesPixelsAlone(shortTable));
}

// The table lookup of Shaders/FramePass.hlsl, line for line.
static float LutChannel(const GammaTransform& transform, float encoded, int channel)
{
	const float position = saturate(encoded) * (GammaLutEntries - 1);
	const uint below = static_cast<uint>(position);
	const uint above = std::min(below + 1, GammaLutEntries - 1);
	const float low = transform.lut[below][channel];
	return low + (transform.lut[above][channel] - low) * (position - below);
}

// The whole pass for an sRGB surface on an SDR wire, as FramePass.hlsl runs it when it doesn't skip the colour work.
static uint FullPass(const GammaTransform& transform, float3 texel, float sdrWhite)
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Sdr, transform);
	float3 encoded = EncodeWire(WIRE_SDR, Through(rows, DecodeSource(SOURCE_SRGB, texel, sdrWhite)), sdrWhite);
	if (transform.lutEnabled)
	{
		encoded = float3(LutChannel(transform, encoded.x, 0), LutChannel(transform, encoded.y, 1), LutChannel(transform, encoded.z, 2));
	}
	return PackXrgb8888(encoded);
}

// What the skip stores must be what the colour work would have, for every 8-bit pixel, with transforms at the edge of what
// LeavesPixelsAlone lets through.
static void Skipping_the_colour_work_gives_the_same_pixels()
{
	GammaTransform both = IdentityMatrix();
	both.lutEnabled = true;
	both.lut = IdentityTable().lut;

	// Every element just inside the matrix's margin, the way that adds most light, and the table just inside its own.
	GammaTransform edgeUp = both;
	GammaTransform edgeDown = both;
	for (int row = 0; row < 3; ++row)
	{
		for (int column = 0; column < 4; ++column)
		{
			edgeUp.matrix[row][column] += 0.95e-5f;
			edgeDown.matrix[row][column] -= row == column ? 0.95e-5f : 0.0f;
		}
	}
	for (auto& entry : edgeUp.lut)
	{
		for (int channel = 0; channel < 3; ++channel)
		{
			entry[channel] += 0.45f / 1023.0f;
		}
	}
	for (auto& entry : edgeDown.lut)
	{
		for (int channel = 0; channel < 3; ++channel)
		{
			entry[channel] -= 0.45f / 1023.0f;
		}
	}

	const GammaTransform transforms[] = {GammaTransform{}, IdentityTable(), IdentityMatrix(), both, edgeUp, edgeDown};
	for (const GammaTransform& transform : transforms)
	{
		CHECK(LeavesPixelsAlone(transform));
		for (float sdrWhite : {1.0f, 2.5f})
		{
			int wrong = 0;
			for (uint red = 0; red < 256; ++red)
			{
				for (uint green = 0; green < 256; green += 5)
				{
					for (uint blue = 0; blue < 256; blue += 3)
					{
						// Each channel takes every value somewhere, with the other two spread over their range.
						const uint values[3][3] = {{red, green, blue}, {blue, red, green}, {green, blue, red}};
						for (const auto& value : values)
						{
							const float3 texel(value[0] / 255.0f, value[1] / 255.0f, value[2] / 255.0f);
							wrong += FullPass(transform, texel, sdrWhite) != PackXrgb8888(texel);
						}
					}
				}
			}
			CHECK(wrong == 0);
		}
	}
}

int main()
{
	Sdr_is_bit_exact_through_the_whole_pipeline();
	Pq_matches_its_published_points();
	Pq_round_trips();
	Scrgb_white_lands_at_the_right_brightness();
	Sdr_content_sits_at_the_sdr_white_level();
	Pq_input_passes_through_unchanged();
	Bt709_to_bt2020_is_the_published_matrix();
	An_identity_transform_changes_nothing();
	The_scalar_multiplies_the_matrix_and_its_offset();
	Channels_land_where_drm_says();
	Out_of_range_never_wraps();
	Only_a_transform_that_changes_nothing_is_skipped();
	Skipping_the_colour_work_gives_the_same_pixels();
	std::printf("colour_math_test: %d failures\n", failures);
	return failures != 0;
}
