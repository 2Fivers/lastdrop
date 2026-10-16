using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

// Attributes a process's private working set (the pages of its working set that are not
// shareable, Task Manager's "Memory") to what holds them: a heap (NT heap segments are found by
// their 0xFFEEFFEE signature), a DLL image (its written data, relocations, IAT), a mapped section,
// a thread stack, a TEB, or another private allocation (big ones are often pixel buffers).
public static class MemMap
{
    [StructLayout(LayoutKind.Sequential)]
    struct MBI
    {
        public IntPtr BaseAddress; public IntPtr AllocationBase; public uint AllocationProtect; public ushort PartitionId;
        public IntPtr RegionSize; public uint State; public uint Protect; public uint Type;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct THREAD_BASIC_INFORMATION
    {
        public int ExitStatus; public IntPtr TebBaseAddress; public IntPtr UniqueProcess; public IntPtr UniqueThread;
        public IntPtr AffinityMask; public int Priority; public int BasePriority;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct HEAPLIST32 { public IntPtr dwSize; public uint th32ProcessID; public UIntPtr th32HeapID; public uint dwFlags; }

    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenThread(uint access, bool inherit, int tid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("psapi.dll", SetLastError = true)] static extern bool QueryWorkingSet(IntPtr h, IntPtr buf, int size);
    [DllImport("kernel32.dll")] static extern IntPtr VirtualQueryEx(IntPtr h, IntPtr addr, out MBI mbi, IntPtr len);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode)] static extern uint GetMappedFileNameW(IntPtr h, IntPtr addr, StringBuilder sb, uint n);
    [DllImport("ntdll.dll")] static extern int NtQueryInformationThread(IntPtr h, int cls, out THREAD_BASIC_INFORMATION tbi, int len, IntPtr ret);
    [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr h, IntPtr addr, byte[] buf, IntPtr n, IntPtr read);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32.dll")] static extern bool Heap32ListFirst(IntPtr snap, ref HEAPLIST32 l);
    [DllImport("kernel32.dll")] static extern bool Heap32ListNext(IntPtr snap, ref HEAPLIST32 l);

    const uint MEM_FREE = 0x10000, MEM_IMAGE = 0x1000000, MEM_MAPPED = 0x40000;
    static readonly IntPtr MbiSize = new IntPtr(Marshal.SizeOf(typeof(MBI)));

    static byte[] Read(IntPtr h, long addr, int n)
    {
        var b = new byte[n];
        return ReadProcessMemory(h, new IntPtr(addr), b, new IntPtr(n), IntPtr.Zero) ? b : null;
    }

    // One line per holder: "category<TAB>name<TAB>private KB<TAB>shareable KB<TAB>allocation KB".
    public static List<string> Map(int pid)
    {
        IntPtr h = OpenProcess(0x0400 | 0x0010 /* QUERY_INFORMATION | VM_READ */, false, pid);
        if (h == IntPtr.Zero) throw new Exception("OpenProcess failed: " + Marshal.GetLastWin32Error());
        try
        {
            // The working set: one entry per page, bit 8 = shareable, bits 12.. = the address.
            var pages = new List<KeyValuePair<long, bool>>();
            for (int size = 1 << 20; ; size *= 2)
            {
                IntPtr buf = Marshal.AllocHGlobal(size);
                try
                {
                    if (!QueryWorkingSet(h, buf, size)) { if (size > (1 << 28)) throw new Exception("QueryWorkingSet failed"); continue; }
                    long n = Marshal.ReadInt64(buf);
                    for (long i = 0; i < n; i++)
                    {
                        long e = Marshal.ReadInt64(buf, (int)(8 + i * 8));
                        pages.Add(new KeyValuePair<long, bool>(e & ~0xFFFL, ((e >> 8) & 1) == 1));
                    }
                    break;
                }
                finally { Marshal.FreeHGlobal(buf); }
            }
            pages.Sort((a, b) => a.Key.CompareTo(b.Key));

            // Heaps: Toolhelp lists them by handle, which is the base of their first segment.
            var heaps = new Dictionary<long, string>();
            IntPtr snap = CreateToolhelp32Snapshot(0x1 /* TH32CS_SNAPHEAPLIST */, (uint)pid);
            if (snap != new IntPtr(-1))
            {
                var hl = new HEAPLIST32 { dwSize = new IntPtr(Marshal.SizeOf(typeof(HEAPLIST32))) };
                int idx = 0;
                if (Heap32ListFirst(snap, ref hl))
                    do { heaps[(long)hl.th32HeapID.ToUInt64()] = "heap#" + idx++ + ((hl.dwFlags & 1) != 0 ? " (process heap)" : ""); }
                    while (Heap32ListNext(snap, ref hl));
                CloseHandle(snap);
            }

            // Thread stacks (from each TEB's StackLimit) and the TEB pages themselves.
            var stacks = new HashSet<long>();
            var tebs = new HashSet<long>();
            foreach (ProcessThread t in Process.GetProcessById(pid).Threads)
            {
                IntPtr th = OpenThread(0x0040 /* QUERY_INFORMATION */, false, t.Id);
                if (th == IntPtr.Zero) continue;
                THREAD_BASIC_INFORMATION tbi;
                if (NtQueryInformationThread(th, 0, out tbi, Marshal.SizeOf(typeof(THREAD_BASIC_INFORMATION)), IntPtr.Zero) == 0)
                {
                    long teb = tbi.TebBaseAddress.ToInt64();
                    tebs.Add(teb & ~0xFFFL);
                    byte[] limit = Read(h, teb + 0x10, 8);
                    MBI m;
                    if (limit != null && VirtualQueryEx(h, new IntPtr(BitConverter.ToInt64(limit, 0)), out m, MbiSize) != IntPtr.Zero)
                        stacks.Add(m.AllocationBase.ToInt64());
                }
                CloseHandle(th);
            }

            var groups = new Dictionary<string, long[]>(); // key -> [private pages, shareable pages, allocation bytes]
            var groupBases = new Dictionary<string, HashSet<long>>();
            var allocSizes = new Dictionary<long, long>();
            MBI cur = new MBI(); long curStart = 0, curEnd = 0, curBase = 0; string curKey = null; long curAlloc = 0;
            foreach (var p in pages)
            {
                if (curKey == null || p.Key < curStart || p.Key >= curEnd)
                {
                    if (VirtualQueryEx(h, new IntPtr(p.Key), out cur, MbiSize) == IntPtr.Zero)
                    {
                        curKey = "system\tno user address (page tables)"; curStart = p.Key; curEnd = p.Key + 4096; curBase = 0; curAlloc = 0;
                    }
                    else
                    {
                        curStart = cur.BaseAddress.ToInt64(); curEnd = curStart + cur.RegionSize.ToInt64();
                        long ab = cur.AllocationBase.ToInt64();
                        curBase = ab;
                        if (!allocSizes.TryGetValue(ab, out curAlloc))
                        {
                            long a = ab; MBI r; curAlloc = 0;
                            while (ab != 0 && VirtualQueryEx(h, new IntPtr(a), out r, MbiSize) != IntPtr.Zero && r.AllocationBase.ToInt64() == ab)
                            { curAlloc += r.RegionSize.ToInt64(); a += r.RegionSize.ToInt64(); }
                            allocSizes[ab] = curAlloc;
                        }
                        string label;
                        if (cur.State == MEM_FREE || ab == 0) curKey = "system\tno user address (page tables)";
                        else if (cur.Type == MEM_IMAGE || cur.Type == MEM_MAPPED)
                        {
                            var sb = new StringBuilder(1024);
                            GetMappedFileNameW(h, cur.AllocationBase, sb, 1024);
                            string name = sb.ToString();
                            name = name.Length == 0 ? "(pagefile-backed section)" : name.Substring(name.LastIndexOf('\\') + 1);
                            curKey = (cur.Type == MEM_IMAGE ? "image\t" : "mapped\t") + name;
                        }
                        else if (heaps.TryGetValue(ab, out label)) curKey = "heap\t" + label;
                        else if (stacks.Contains(ab)) curKey = "stack\tthread stacks";
                        else
                        {
                            // An NT heap segment: signature 0xFFEEFFEE at +0x10, its heap at +0x28.
                            byte[] hdr = Read(h, ab, 0x30);
                            if (hdr != null && BitConverter.ToUInt32(hdr, 0x10) == 0xFFEEFFEE && heaps.TryGetValue(BitConverter.ToInt64(hdr, 0x28), out label))
                                curKey = "heap\t" + label;
                            else
                                curKey = "private\t0x" + ab.ToString("x") + " (" + (curAlloc >> 10) + " KB allocation)";
                        }
                    }
                }
                string key = curKey.StartsWith("private\t") && tebs.Contains(p.Key) ? "teb\tthread environment blocks" : curKey;
                long[] g;
                if (!groups.TryGetValue(key, out g)) { g = new long[3]; groups[key] = g; groupBases[key] = new HashSet<long>(); }
                if (curBase != 0 && groupBases[key].Add(curBase)) g[2] += curAlloc;
                if (p.Value) g[1]++; else g[0]++;
            }
            var lines = new List<string>();
            foreach (var kv in groups)
                lines.Add(kv.Key + "\t" + (kv.Value[0] * 4) + "\t" + (kv.Value[1] * 4) + "\t" + (kv.Value[2] >> 10));
            return lines;
        }
        finally { CloseHandle(h); }
    }
}
