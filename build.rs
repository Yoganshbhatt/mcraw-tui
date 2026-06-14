fn main() {
    #[cfg(windows)]
    {
        unsafe {
            extern "system" {
                fn SetErrorMode(mode: u32) -> u32;
            }
            // Suppress Windows error popups for child processes (broken GCC
            // LTO plugin in bfd-plugins/libep.a triggering WerFault dialogs
            // during linking). Inherited by rustc → linker → plugin process.
            const SEM_FAILCRITICALERRORS: u32 = 0x0001;
            const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
            const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
    }
}
