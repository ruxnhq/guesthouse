//! nvapi64.dll for a Windows guest on a GPU-PV card.
//!
//! NVIDIA's NVAPI goes into a mode of its own when it sees a paravirtualized NVIDIA adapter, as
//! every GPU-PV guest has, and then refuses NvAPI_GPU_GetLogicalGpuInfo. NGX, which runs DLSS,
//! needs that call to match the card to the game's Direct3D device, so DLSS finds no card. This
//! DLL sits in System32 under NVIDIA's name, loads NVIDIA's own from the driver's folder, and hands
//! it every call. It answers only that one and GetAdapterIdFromPhysicalGpu itself, and only when
//! NVIDIA's fails, with the card's physical GPU and the guest adapter's LUID.
//!
//! It must never leave a program worse off than NVIDIA's DLL alone. Whatever it can't work out, it
//! passes NVIDIA's answer through, and with no NVIDIA DLL to load it answers as if NVAPI weren't
//! there.

pub mod logic;

#[cfg(windows)]
mod windows;
