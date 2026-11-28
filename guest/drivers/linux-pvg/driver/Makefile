# SPDX-License-Identifier: GPL-2.0
obj-m := pvg.o
pvg-y := pvg_pci.o pvg_ring.o pvg_debugfs.o pvg_kms.o pvg_efb.o \
	pvg_surface.o pvg_display.o pvg_mapper.o pvg_gem.o pvg_bo.o pvg_task.o pvg_child.o
# The interface for programs is beside the driver, and includes DRM's own as
# programs do.
ccflags-y += -I$(src)/../uapi -I$(srctree)/include/uapi/drm
