//! Implémentations Windows (API Win32 via `windows-sys`).
//!
//! Contenu :
//! - [`JobObject`] : « conteneur » de processus. Tous les services y sont placés ;
//!   si Rustserv se termine (même brutalement), Windows tue tout le contenu :
//!   aucun processus orphelin ne garde les ports 80/3306 occupés.
//! - [`spawn_in_job`] : lance un processus **suspendu**, l'ajoute au Job, puis le
//!   relance. Ainsi même les sous-processus créés immédiatement (workers nginx)
//!   sont dans le Job.
//! - [`port_owner`] : quel processus écoute sur un port TCP.
//! - [`signal_named_event`] : arrêt propre d'Apache / MariaDB / nginx, qui
//!   surveillent un événement Windows nommé d'après leur PID.
//! - [`vcruntime_version`] : version de vcruntime140.dll (runtime Visual C++).

#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    OpenEventW, OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread, SetEvent, EVENT_MODIFY_STATE,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

/// Drapeaux de création de processus.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const CREATE_SUSPENDED: u32 = 0x0000_0004;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Nom de l'exécutable avec l'extension propre à la plateforme.
pub fn exe_name(base: &str) -> String {
    format!("{base}.exe")
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Handle Windows fermé automatiquement.
struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ------------------------------------------------------------------ Job Object

pub struct JobObject(Owned);

// Le handle d'un Job peut être utilisé depuis n'importe quel thread.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// Crée un Job qui tue tous ses processus quand son dernier handle est fermé
    /// (fermeture de Rustserv, plantage compris).
    pub fn new() -> std::io::Result<Self> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = JobObject(Owned(h));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    fn assign(&self, process: HANDLE) -> std::io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0 .0, process) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Lance `cmd` dans le Job : création suspendue → ajout au Job → reprise.
/// Les drapeaux CREATE_NO_WINDOW et CREATE_NEW_PROCESS_GROUP sont ajoutés.
pub fn spawn_in_job(job: &JobObject, cmd: &mut tokio::process::Command) -> std::io::Result<tokio::process::Child> {
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP);
    let child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);
    let handle = child.raw_handle().map(|h| h as HANDLE);
    let assign = match handle {
        Some(h) => job.assign(h),
        None => Err(std::io::Error::other("handle de processus indisponible")),
    };
    // On relance le processus même si l'ajout au Job a échoué, sinon il resterait figé.
    let resume = resume_process(pid);
    if let Err(e) = assign {
        tracing::warn!(pid, erreur = %e, "impossible d'ajouter le processus au Job");
    }
    resume?;
    Ok(child)
}

/// Relance tous les threads d'un processus créé suspendu.
fn resume_process(pid: u32) -> std::io::Result<()> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let snap = Owned(snap);
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = false;
        let mut ok = Thread32First(snap.0, &mut te);
        while ok != 0 {
            if te.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                if !t.is_null() {
                    let t = Owned(t);
                    ResumeThread(t.0);
                    found = true;
                }
            }
            ok = Thread32Next(snap.0, &mut te);
        }
        if found { Ok(()) } else { Err(std::io::Error::other(format!("aucun thread trouvé pour le PID {pid}"))) }
    }
}

// ------------------------------------------------------------------ Ports

/// Processus qui écoute sur un port TCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortOwner {
    pub pid: u32,
    /// Nom de l'exécutable (ex. `httpd.exe`), si accessible.
    pub name: Option<String>,
    pub path: Option<PathBuf>,
}

/// Cherche le processus qui écoute sur `port` (IPv4 puis IPv6).
pub fn port_owner(port: u16) -> Option<PortOwner> {
    let pid = listening_pid(port, AF_INET as u32).or_else(|| listening_pid(port, AF_INET6 as u32))?;
    let path = process_path(pid);
    let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).or_else(|| {
        // Processus système (PID 4 = noyau, ex. http.sys / IIS / partage).
        if pid == 4 { Some("System (http.sys)".into()) } else { None }
    });
    Some(PortOwner { pid, name, path })
}

fn listening_pid(port: u16, family: u32) -> Option<u32> {
    unsafe {
        let mut size: u32 = 0;
        GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0);
        if size == 0 {
            return None;
        }
        // Tampon aligné sur 4 octets (u32) comme l'exige la structure.
        let mut buf = vec![0u32; (size as usize).div_ceil(4) + 1];
        if GetExtendedTcpTable(buf.as_mut_ptr() as *mut _, &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0) != 0 {
            return None;
        }
        // Le port est stocké en ordre réseau (big-endian) dans les 16 bits de poids faible.
        let net = |p: u32| u16::from_be(p as u16);
        if family == AF_INET as u32 {
            let t = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
            let rows = std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize);
            rows.iter().find(|r| net(r.dwLocalPort) == port).map(|r| r.dwOwningPid)
        } else {
            let t = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
            let rows = std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize);
            rows.iter().find(|r| net(r.dwLocalPort) == port).map(|r| r.dwOwningPid)
        }
    }
}

/// Chemin complet de l'exécutable d'un processus (None si accès refusé).
pub fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let h = Owned(h);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

// ------------------------------------------------------------------ Arrêt propre

/// Déclenche l'événement Windows nommé `name` (ex. `MySQLShutdown1234`).
/// Renvoie `false` si l'événement n'existe pas (processus pas encore prêt ou déjà arrêté).
pub fn signal_named_event(name: &str) -> bool {
    unsafe {
        let w = wide(name);
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr());
        if h.is_null() {
            return false;
        }
        let h = Owned(h);
        SetEvent(h.0) != 0
    }
}

// ------------------------------------------------------------------ Runtime VC++

/// Version de `System32\vcruntime140.dll` (ex. « 14.51.36247.0 »), ou None si absent.
pub fn vcruntime_version() -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    unsafe {
        let mut dir = [0u16; 260];
        let n = GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) as usize;
        if n == 0 {
            return None;
        }
        let path = format!("{}\\vcruntime140.dll", String::from_utf16_lossy(&dir[..n]));
        let wpath = wide(&path);
        let size = GetFileVersionInfoSizeW(wpath.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wpath.as_ptr(), 0, size, data.as_mut_ptr() as *mut _) == 0 {
            return None;
        }
        let mut info: *mut VS_FIXEDFILEINFO = std::ptr::null_mut();
        let mut len: u32 = 0;
        let root = wide("\\");
        if VerQueryValueW(data.as_ptr() as *const _, root.as_ptr(), &mut info as *mut _ as *mut _, &mut len) == 0
            || info.is_null()
        {
            return None;
        }
        let i = &*info;
        Some(format!(
            "{}.{}.{}.{}",
            i.dwFileVersionMS >> 16,
            i.dwFileVersionMS & 0xffff,
            i.dwFileVersionLS >> 16,
            i.dwFileVersionLS & 0xffff
        ))
    }
}

// ------------------------------------------------------------------ Élévation UAC

/// Résultat d'un lancement en administrateur.
#[derive(Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Le programme s'est exécuté ; code de sortie.
    Termine(u32),
    /// L'utilisateur a refusé la demande de Windows.
    Refuse,
}

/// Lance `exe args` **en administrateur** (fenêtre UAC) et attend la fin.
pub fn run_elevated(exe: &std::path::Path, args: &str) -> std::io::Result<Elevation> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = wide("runas");
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let params = wide(args);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            if GetLastError() == ERROR_CANCELLED {
                return Ok(Elevation::Refuse);
            }
            return Err(std::io::Error::last_os_error());
        }
        if info.hProcess.is_null() {
            return Err(std::io::Error::other("processus élevé introuvable"));
        }
        let h = Owned(info.hProcess);
        WaitForSingleObject(h.0, INFINITE);
        let mut code: u32 = 0;
        GetExitCodeProcess(h.0, &mut code);
        Ok(Elevation::Termine(code))
    }
}

// ------------------------------------------------------------------ Certificats

/// Magasin « Autorités de certification racines de confiance » de l'utilisateur courant
/// (pas besoin des droits administrateur ; Windows demande une confirmation à l'ajout).
struct RootStore(windows_sys::Win32::Security::Cryptography::HCERTSTORE);

impl RootStore {
    fn open() -> std::io::Result<Self> {
        use windows_sys::Win32::Security::Cryptography::{CertOpenStore, CERT_STORE_PROV_SYSTEM_W, CERT_SYSTEM_STORE_CURRENT_USER};
        let name = wide("Root");
        let h = unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER, name.as_ptr() as *const _) };
        if h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(h))
    }

    /// Cherche un certificat identique (encodage DER) ; renvoie son contexte (à libérer).
    fn find(&self, der: &[u8]) -> Option<*const windows_sys::Win32::Security::Cryptography::CERT_CONTEXT> {
        use windows_sys::Win32::Security::Cryptography::{
            CertCreateCertificateContext, CertFindCertificateInStore, CertFreeCertificateContext, CERT_FIND_EXISTING, PKCS_7_ASN_ENCODING,
            X509_ASN_ENCODING,
        };
        unsafe {
            let ctx = CertCreateCertificateContext(X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, der.as_ptr(), der.len() as u32);
            if ctx.is_null() {
                return None;
            }
            let found = CertFindCertificateInStore(
                self.0,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_EXISTING,
                ctx as *const _,
                std::ptr::null(),
            );
            CertFreeCertificateContext(ctx);
            (!found.is_null()).then_some(found)
        }
    }
}

impl Drop for RootStore {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Security::Cryptography::CertCloseStore(self.0, 0) };
    }
}

/// Le certificat (DER) est-il dans les autorités racines de l'utilisateur ?
pub fn root_cert_installed(der: &[u8]) -> bool {
    let Ok(store) = RootStore::open() else { return false };
    match store.find(der) {
        Some(c) => {
            unsafe { windows_sys::Win32::Security::Cryptography::CertFreeCertificateContext(c) };
            true
        }
        None => false,
    }
}

/// Ajoute le certificat aux autorités racines de l'utilisateur.
/// Windows affiche une fenêtre de confirmation : renvoie `Ok(false)` si l'utilisateur refuse.
pub fn install_root_cert(der: &[u8]) -> std::io::Result<bool> {
    use windows_sys::Win32::Security::Cryptography::{CertAddEncodedCertificateToStore, CERT_STORE_ADD_REPLACE_EXISTING, X509_ASN_ENCODING};
    let store = RootStore::open()?;
    let ok = unsafe {
        CertAddEncodedCertificateToStore(store.0, X509_ASN_ENCODING, der.as_ptr(), der.len() as u32, CERT_STORE_ADD_REPLACE_EXISTING, std::ptr::null_mut())
    };
    if ok == 0 {
        let e = std::io::Error::last_os_error();
        // 0x800704C7 = ERROR_CANCELLED : bouton « Non » de la confirmation Windows.
        if e.raw_os_error() == Some(0x800704C7u32 as i32) || e.raw_os_error() == Some(1223) {
            return Ok(false);
        }
        return Err(e);
    }
    Ok(true)
}

/// Retire le certificat des autorités racines (Windows peut demander confirmation).
pub fn remove_root_cert(der: &[u8]) -> std::io::Result<bool> {
    use windows_sys::Win32::Security::Cryptography::CertDeleteCertificateFromStore;
    let store = RootStore::open()?;
    match store.find(der) {
        // CertDeleteCertificateFromStore libère le contexte.
        Some(c) => Ok(unsafe { CertDeleteCertificateFromStore(c) } != 0),
        None => Ok(false),
    }
}

/// Récupère le handle brut d'un `std::process::Child` (utile hors tokio).
#[allow(dead_code)]
pub fn raw_handle(child: &std::process::Child) -> HANDLE {
    child.as_raw_handle() as HANDLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trouve_le_proprietaire_d_un_port() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let owner = port_owner(port).expect("port en écoute non détecté");
        assert_eq!(owner.pid, std::process::id());
        assert!(owner.name.unwrap().to_lowercase().ends_with(".exe"));
        drop(l);
        assert!(port_owner(port).is_none());
    }

    #[test]
    fn version_vcruntime_lisible() {
        // Présent sur quasiment toutes les machines Windows ; on vérifie juste le format.
        if let Some(v) = vcruntime_version() {
            assert!(v.starts_with("14."), "{v}");
        }
    }

    #[test]
    fn evenement_inexistant() {
        assert!(!signal_named_event("RustservEvenementQuiNExistePas"));
    }
}
