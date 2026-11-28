#include "check.h"
#include "../GuesthouseDisplay/Guesthouse/PassGeometry.h"
#include "../GuesthouseDisplay/Guesthouse/WireFormat.h"

#include <vector>

using namespace Guesthouse;

// The copy layout of Shaders/FramePass.hlsl, line for line: counts each pixel's stores, and checks each 16-byte store stays in
// its row's pitch.
static void Copy_writes_each_pixel_once_and_stays_in_the_row(std::uint32_t width, std::uint32_t height, std::uint32_t rows)
{
	const std::uint32_t pitch = PitchFor(width);
	std::vector<int> stores(static_cast<size_t>(pitch / 4) * height, 0);
	bool inside = true;

	const PassGroups groups = CopyGroups(width, height, rows);
	for (std::uint32_t gy = 0; gy < groups.y; ++gy)
	{
		for (std::uint32_t gx = 0; gx < groups.x; ++gx)
		{
			for (std::uint32_t index = 0; index < PassGroupSide * PassGroupSide; ++index)
			{
				const std::uint32_t column = (gx * PassGroupSide * PassGroupSide + index) * CopyPixelsPerThread;
				if (column >= width)
				{
					continue;
				}
				const std::uint32_t top = gy * rows;
				const std::uint32_t bottom = std::min(top + rows, height);
				for (std::uint32_t y = top; y < bottom; ++y)
				{
					const std::uint32_t address = y * pitch + column * 4;
					inside = inside && address % 16 == 0 && address + 16 <= (y + 1) * pitch;
					for (std::uint32_t word = 0; word < 4 && address / 4 + word < stores.size(); ++word)
					{
						++stores[address / 4 + word];
					}
				}
			}
		}
	}

	CHECK(inside);
	bool once = true;
	for (std::uint32_t y = 0; y < height; ++y)
	{
		for (std::uint32_t x = 0; x < pitch / 4; ++x)
		{
			const int count = stores[static_cast<size_t>(y) * (pitch / 4) + x];
			once = once && (x < width ? count == 1 : count <= 1);
		}
	}
	if (!once)
	{
		std::printf("  copy layout %ux%u, %u rows\n", width, height, rows);
	}
	CHECK(once);
}

static void The_copy_layout_covers_every_mode()
{
	const std::uint32_t sizes[][2] = {{640, 480}, {1024, 768}, {1280, 720}, {1366, 768}, {1920, 1080}, {2560, 1440},
	                                  {3440, 1440}, {3840, 2160}, {1, 1}, {257, 3}, {255, 9}};
	const std::uint32_t rowCounts[] = {1, 2, 7, 8, 32, MostCopyRows};
	for (const auto& size : sizes)
	{
		for (std::uint32_t rows : rowCounts)
		{
			Copy_writes_each_pixel_once_and_stays_in_the_row(size[0], size[1], rows);
		}
	}
}

static void Fewer_groups_than_the_normal_layout()
{
	const PassGroups normal = NormalGroups(1920, 1080);
	const PassGroups copy = CopyGroups(1920, 1080, DefaultCopyRows);
	CHECK(normal.x * normal.y == 240u * 135u);
	CHECK(copy.x * copy.y == 8u * 135u);
}

// The cursor's run of the normal layout, with its origin: every on-screen pixel of the cursor gets a thread, and none lands off
// the screen.
static void The_cursor_run_covers_the_visible_cursor()
{
	const std::uint32_t width = 1920;
	const std::uint32_t height = 1080;
	const std::int32_t places[][2] = {{0, 0}, {100, 200}, {-10, -20}, {1900, 1070}, {-31, 500}, {1919, 1079}, {-64, -64},
	                                  {1920, 0}, {0, 1080}, {5000, 5000}, {-5000, 10}};
	const std::uint32_t shapes[][2] = {{32, 32}, {64, 64}, {48, 33}, {256, 256}, {1, 1}};
	for (const auto& place : places)
	{
		for (const auto& shape : shapes)
		{
			const PassArea area = CursorArea(place[0], place[1], shape[0], shape[1], width, height);
			std::vector<char> hit(static_cast<size_t>(width) * height, 0);
			const PassGroups groups = NormalGroups(area.width, area.height);
			for (std::uint32_t gy = 0; gy < groups.y; ++gy)
			{
				for (std::uint32_t gx = 0; gx < groups.x; ++gx)
				{
					for (std::uint32_t t = 0; t < PassGroupSide * PassGroupSide; ++t)
					{
						const std::uint32_t x = gx * PassGroupSide + t % PassGroupSide + area.left;
						const std::uint32_t y = gy * PassGroupSide + t / PassGroupSide + area.top;
						if (x < width && y < height)
						{
							hit[static_cast<size_t>(y) * width + x] = 1;
						}
					}
				}
			}

			bool covered = true;
			std::uint32_t visible = 0;
			for (std::int64_t y = place[1]; y < std::int64_t{place[1]} + shape[1]; ++y)
			{
				for (std::int64_t x = place[0]; x < std::int64_t{place[0]} + shape[0]; ++x)
				{
					if (x >= 0 && y >= 0 && x < width && y < height)
					{
						++visible;
						covered = covered && hit[static_cast<size_t>(y) * width + static_cast<size_t>(x)];
					}
				}
			}
			CHECK(covered);
			CHECK(visible == area.width * area.height);
		}
	}
}

int main()
{
	The_copy_layout_covers_every_mode();
	Fewer_groups_than_the_normal_layout();
	The_cursor_run_covers_the_visible_cursor();
	std::printf("pass_geometry_test: %d failures\n", failures);
	return failures != 0;
}
