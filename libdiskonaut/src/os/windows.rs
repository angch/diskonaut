#[cfg(not(test))]
pub fn is_user_admin() -> bool {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct SID_IDENTIFIER_AUTHORITY {
        Value: [u8; 6],
    }

    const SECURITY_NT_AUTHORITY: SID_IDENTIFIER_AUTHORITY = SID_IDENTIFIER_AUTHORITY {
        Value: [0, 0, 0, 0, 0, 5],
    };
    const SECURITY_BUILTIN_DOMAIN_RID: u32 = 0x0000_0020;
    const DOMAIN_ALIAS_RID_ADMINS: u32 = 0x0000_0220;

    #[link(name = "advapi32")]
    #[allow(non_snake_case)]
    unsafe extern "system" {
        fn AllocateAndInitializeSid(
            pIdentifierAuthority: *const SID_IDENTIFIER_AUTHORITY,
            nSubAuthorityCount: u8,
            nSubAuthority0: u32,
            nSubAuthority1: u32,
            nSubAuthority2: u32,
            nSubAuthority3: u32,
            nSubAuthority4: u32,
            nSubAuthority5: u32,
            nSubAuthority6: u32,
            nSubAuthority7: u32,
            pSid: *mut *mut core::ffi::c_void,
        ) -> i32;

        fn CheckTokenMembership(
            TokenHandle: *mut core::ffi::c_void,
            SidToCheck: *mut core::ffi::c_void,
            IsMember: *mut i32,
        ) -> i32;

        fn FreeSid(pSid: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    }

    let mut admin_sid: *mut core::ffi::c_void = core::ptr::null_mut();
    let auth = SECURITY_NT_AUTHORITY;
    let alloc_success = unsafe {
        AllocateAndInitializeSid(
            &auth,
            2,
            SECURITY_BUILTIN_DOMAIN_RID,
            DOMAIN_ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut admin_sid,
        )
    };

    if alloc_success == 0 {
        return false;
    }

    let mut is_member: i32 = 0;
    let check_success =
        unsafe { CheckTokenMembership(core::ptr::null_mut(), admin_sid, &mut is_member) };

    unsafe {
        FreeSid(admin_sid);
    }

    check_success != 0 && is_member != 0
}

#[cfg(test)]
pub fn is_user_admin() -> bool {
    false
}

pub fn size_on_disk_fast(metadata: &::std::fs::Metadata) -> u64 {
    metadata.len()
}

#[repr(C)]
#[derive(Default)]
#[allow(non_snake_case)]
struct BY_HANDLE_FILE_INFORMATION {
    dwFileAttributes: u32,
    ftCreationTime: [u32; 2],
    ftLastAccessTime: [u32; 2],
    ftLastWriteTime: [u32; 2],
    dwVolumeSerialNumber: u32,
    nFileSizeHigh: u32,
    nFileSizeLow: u32,
    nNumberOfLinks: u32,
    nFileIndexHigh: u32,
    nFileIndexLow: u32,
}

fn query_file_info(path: &::std::path::Path) -> Option<BY_HANDLE_FILE_INFORMATION> {
    use ::std::os::windows::ffi::OsStrExt;

    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    const FILE_SHARE_DELETE: u32 = 4;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const INVALID_HANDLE_VALUE: *mut core::ffi::c_void = -1isize as *mut core::ffi::c_void;

    #[link(name = "kernel32")]
    #[allow(non_snake_case)]
    unsafe extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut core::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void;

        fn GetFileInformationByHandle(
            hFile: *mut core::ffi::c_void,
            lpFileInformation: *mut BY_HANDLE_FILE_INFORMATION,
        ) -> i32;

        fn CloseHandle(hObject: *mut core::ffi::c_void) -> i32;
    }

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);

    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            core::ptr::null_mut(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe {
        CloseHandle(handle);
    }

    if ok != 0 { Some(info) } else { None }
}

pub fn volume_id(path: &::std::path::Path) -> Option<u64> {
    query_file_info(path).map(|info| u64::from(info.dwVolumeSerialNumber))
}

pub fn link_count(path: &::std::path::Path) -> u64 {
    query_file_info(path)
        .map(|info| u64::from(info.nNumberOfLinks))
        .unwrap_or(1)
}
