// NetworkIsolation ownership and structure layout follow the Windows SDK:
// https://learn.microsoft.com/windows/win32/api/netfw/nf-netfw-networkisolationgetappcontainerconfig
// https://learn.microsoft.com/windows/win32/api/networkisolation/ns-networkisolation-inet_firewall_app_container
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
using System.Threading;

[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.System32)]

namespace Transmog {
    public sealed class AppContainerRow {
        public string displayName;
        public string description;
        public string package;
        public string name;
        public string sid;
        public string[] users;
        public bool exempt;
    }

    public static class Loopback {
        [StructLayout(LayoutKind.Sequential)]
        private struct SidAttributes { public IntPtr Sid; public uint Attributes; }
        [StructLayout(LayoutKind.Sequential)]
        private struct PointerList { public uint Count; public IntPtr Items; }
        [StructLayout(LayoutKind.Sequential)]
        private struct Container {
            public IntPtr Sid, UserSid, Name, DisplayName, Description;
            public PointerList Capabilities, Binaries;
            public IntPtr WorkingDirectory, Package;
        }
        [DllImport("FirewallAPI.dll")] private static extern uint NetworkIsolationEnumAppContainers(uint flags, out uint count, out IntPtr containers);
        [DllImport("FirewallAPI.dll")] private static extern uint NetworkIsolationFreeAppContainers(IntPtr containers);
        [DllImport("FirewallAPI.dll")] private static extern uint NetworkIsolationGetAppContainerConfig(out uint count, out IntPtr sids);
        [DllImport("FirewallAPI.dll")] private static extern uint NetworkIsolationSetAppContainerConfig(uint count, [In] SidAttributes[] sids);
        [DllImport("kernel32.dll")] private static extern IntPtr GetProcessHeap();
        [DllImport("kernel32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool HeapFree(IntPtr heap, uint flags, IntPtr memory);
        [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)] private static extern int SHLoadIndirectString(string source, StringBuilder destination, uint length, IntPtr reserved);

        private static void Check(uint result) { if (result != 0) throw new Win32Exception((int)result); }
        private static string Text(IntPtr pointer) {
            string text = pointer == IntPtr.Zero ? "" : Marshal.PtrToStringUni(pointer) ?? "";
            return text.Length <= 4096 ? text : text.Substring(0, 4096);
        }
        private static string Friendly(IntPtr pointer) {
            string text = Text(pointer);
            if (!text.StartsWith("@", StringComparison.Ordinal)) return text;
            var buffer = new StringBuilder(4096);
            return SHLoadIndirectString(text, buffer, 4096, IntPtr.Zero) == 0 ? buffer.ToString() : "";
        }
        private static string Sid(IntPtr pointer) { return new SecurityIdentifier(pointer).Value; }
        private static string User(IntPtr pointer) {
            if (pointer == IntPtr.Zero) return "";
            var sid = new SecurityIdentifier(pointer);
            try { return sid.Translate(typeof(NTAccount)).Value + " (" + sid.Value + ")"; }
            catch (IdentityNotMappedException) { return sid.Value; }
        }
        private static Dictionary<string, uint> Configuration() {
            uint count; IntPtr pointer;
            Check(NetworkIsolationGetAppContainerConfig(out count, out pointer));
            try {
                if (count > 50000) throw new InvalidOperationException("Too many configured exemptions.");
                int size = Marshal.SizeOf(typeof(SidAttributes));
                var result = new Dictionary<string, uint>(StringComparer.Ordinal);
                for (int i = 0; i < count; i++) {
                    var item = (SidAttributes)Marshal.PtrToStructure(IntPtr.Add(pointer, checked(i * size)), typeof(SidAttributes));
                    result[Sid(item.Sid)] = item.Attributes;
                }
                return result;
            } finally {
                int size = Marshal.SizeOf(typeof(SidAttributes));
                for (int i = 0; i < count; i++) {
                    var item = (SidAttributes)Marshal.PtrToStructure(IntPtr.Add(pointer, checked(i * size)), typeof(SidAttributes));
                    if (item.Sid != IntPtr.Zero) HeapFree(GetProcessHeap(), 0, item.Sid);
                }
                if (pointer != IntPtr.Zero) HeapFree(GetProcessHeap(), 0, pointer);
            }
        }
        private static List<AppContainerRow> Containers(bool metadata) {
            uint count; IntPtr pointer;
            Check(NetworkIsolationEnumAppContainers(0, out count, out pointer));
            try {
                if (count > 50000) throw new InvalidOperationException("Too many AppContainers.");
                int size = Marshal.SizeOf(typeof(Container));
                var rows = new Dictionary<string, AppContainerRow>(StringComparer.Ordinal);
                var users = new Dictionary<string, string>(StringComparer.Ordinal);
                for (int i = 0; i < count; i++) {
                    var item = (Container)Marshal.PtrToStructure(IntPtr.Add(pointer, checked(i * size)), typeof(Container));
                    string sid = Sid(item.Sid), user = "";
                    if (!metadata) {
                        if (!rows.ContainsKey(sid)) rows.Add(sid, new AppContainerRow { sid = sid });
                        continue;
                    }
                    if (item.UserSid != IntPtr.Zero) {
                        string userSid = Sid(item.UserSid);
                        if (!users.TryGetValue(userSid, out user)) { user = User(item.UserSid); users.Add(userSid, user); }
                    }
                    AppContainerRow row;
                    if (!rows.TryGetValue(sid, out row)) {
                        string name = Text(item.Name), display = Friendly(item.DisplayName);
                        row = new AppContainerRow { sid = sid, name = name, displayName = display.Length > 0 ? display : name.Length > 0 ? name : sid,
                            description = Friendly(item.Description), package = Text(item.Package), users = new string[0] };
                        rows.Add(sid, row);
                    }
                    if (user.Length > 0) row.users = row.users.Concat(new[] {user}).Distinct().OrderBy(value => value, StringComparer.OrdinalIgnoreCase).ToArray();
                }
                return rows.Values.ToList();
            } finally { if (pointer != IntPtr.Zero) NetworkIsolationFreeAppContainers(pointer); }
        }
        public static AppContainerRow[] Read() {
            var rows = Containers(true);
            var exemptions = Configuration();
            var known = new HashSet<string>(rows.Select(row => row.sid), StringComparer.Ordinal);
            foreach (var sid in exemptions.Keys) {
                if (!known.Contains(sid)) rows.Add(new AppContainerRow { sid = sid, displayName = "Unavailable AppContainer",
                    description = "A saved loopback exemption remains, but installed metadata is unavailable.", name = "", package = "", users = new string[0] });
            }
            foreach (var row in rows) row.exempt = exemptions.ContainsKey(row.sid);
            return rows.OrderBy(row => row.displayName, StringComparer.OrdinalIgnoreCase).ThenBy(row => row.sid, StringComparer.Ordinal).ToArray();
        }

        // This pure planning function is also exercised with fixture configurations.
        // Preserve every unselected SID and its original attributes.
        public static Dictionary<string, uint> Merge(Dictionary<string, uint> current, string[] selected, bool enabled, HashSet<string> installed) {
            if (selected == null || selected.Length == 0 || selected.Length > 4096 || selected.Distinct().Count() != selected.Length)
                throw new ArgumentException("Choose between 1 and 4096 different AppContainers.");
            var result = new Dictionary<string, uint>(current, StringComparer.Ordinal);
            foreach (string sid in selected) {
                var canonical = new SecurityIdentifier(sid);
                if (!sid.StartsWith("S-1-15-2-", StringComparison.Ordinal) || canonical.Value != sid)
                    throw new ArgumentException("Invalid AppContainer SID.");
                if (enabled) {
                    if (!installed.Contains(sid)) throw new ArgumentException("An AppContainer is no longer installed. Refresh and try again.");
                    if (!result.ContainsKey(sid)) result.Add(sid, 0);
                } else result.Remove(sid);
            }
            return result;
        }
        public static void Apply(string[] selected, bool enabled) {
            using (var gate = new Mutex(false, @"Global\Transmog.LoopbackExemptions")) {
                bool held = false;
                try {
                    try { held = gate.WaitOne(TimeSpan.FromSeconds(30)); } catch (AbandonedMutexException) { held = true; }
                    if (!held) throw new InvalidOperationException("Another exemption change is in progress. Try again.");
                    // Elevated validation needs only identities, never user-controlled
                    // resource paths or account-name lookups from package metadata.
                    var installed = new HashSet<string>(Containers(false).Select(row => row.sid), StringComparer.Ordinal);
                    var current = Configuration();
                    var desired = Merge(current, selected, enabled, installed);
                    if (current.Count == desired.Count && current.All(item => desired.ContainsKey(item.Key) && desired[item.Key] == item.Value)) return;
                    var allocated = new List<IntPtr>();
                    try {
                        var entries = desired.Select(item => {
                            var sid = new SecurityIdentifier(item.Key);
                            var bytes = new byte[sid.BinaryLength]; sid.GetBinaryForm(bytes, 0);
                            var pointer = Marshal.AllocHGlobal(bytes.Length); allocated.Add(pointer); Marshal.Copy(bytes, 0, pointer, bytes.Length);
                            return new SidAttributes { Sid = pointer, Attributes = item.Value };
                        }).ToArray();
                        Check(NetworkIsolationSetAppContainerConfig((uint)entries.Length, entries));
                    } finally { foreach (IntPtr pointer in allocated) Marshal.FreeHGlobal(pointer); }
                    var verified = Configuration();
                    if (selected.Any(sid => verified.ContainsKey(sid) != enabled)) throw new InvalidOperationException("Windows did not retain the requested exemption change. Refresh to inspect the current state.");
                } finally { if (held) gate.ReleaseMutex(); }
            }
        }
    }
}
