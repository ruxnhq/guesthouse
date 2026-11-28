#pragma once

// How many groups each run of Shaders/FramePass.hlsl takes, and where. No D3D here, so the tests can check it on Linux.

#include <algorithm>
#include <cstdint>

namespace Guesthouse
{

// Keep in step with the shader's GROUP_SIZE and COPY_PIXELS.
constexpr std::uint32_t PassGroupSide = 8;
constexpr std::uint32_t CopyPixelsPerThread = 4;
constexpr std::uint32_t CopySpan = PassGroupSide * PassGroupSide * CopyPixelsPerThread;

// Rows each copy group moves. Eight makes a group cover 2048 pixels, against 64 for the normal layout. The 2026-11-10 lab ran an
// empty dispatch in 579 us with a group per 64 pixels and in 322 us with a group per 2048.
constexpr std::uint32_t DefaultCopyRows = 8;
constexpr std::uint32_t MostCopyRows = 64;

struct PassGroups
{
	std::uint32_t x = 0;
	std::uint32_t y = 0;
};

inline std::uint32_t RoundUpDivide(std::uint32_t value, std::uint32_t by)
{
	return (value + by - 1) / by;
}

inline PassGroups NormalGroups(std::uint32_t width, std::uint32_t height)
{
	return {RoundUpDivide(width, PassGroupSide), RoundUpDivide(height, PassGroupSide)};
}

inline PassGroups CopyGroups(std::uint32_t width, std::uint32_t height, std::uint32_t rows)
{
	return {RoundUpDivide(width, CopySpan), RoundUpDivide(height, rows)};
}

// The part of the cursor that is on the screen. Empty if none is.
struct PassArea
{
	std::uint32_t left = 0;
	std::uint32_t top = 0;
	std::uint32_t width = 0;
	std::uint32_t height = 0;
};

inline PassArea CursorArea(std::int32_t x, std::int32_t y, std::uint32_t cursorWidth, std::uint32_t cursorHeight,
                           std::uint32_t width, std::uint32_t height)
{
	const std::int64_t left = (std::max<std::int64_t>)(x, 0);
	const std::int64_t top = (std::max<std::int64_t>)(y, 0);
	const std::int64_t right = (std::min<std::int64_t>)(std::int64_t{x} + cursorWidth, width);
	const std::int64_t bottom = (std::min<std::int64_t>)(std::int64_t{y} + cursorHeight, height);
	if (right <= left || bottom <= top)
	{
		return {};
	}
	return {static_cast<std::uint32_t>(left), static_cast<std::uint32_t>(top), static_cast<std::uint32_t>(right - left),
	        static_cast<std::uint32_t>(bottom - top)};
}

}
