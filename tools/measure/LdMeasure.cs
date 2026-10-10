using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class LdMeasure
{
    [StructLayout(LayoutKind.Sequential)]
    public struct PMC2
    {
        public uint cb;
        public uint PageFaultCount;
        public UIntPtr PeakWorkingSetSize;
        public UIntPtr WorkingSetSize;
        public UIntPtr QuotaPeakPagedPoolUsage;
        public UIntPtr QuotaPagedPoolUsage;
        public UIntPtr QuotaPeakNonPagedPoolUsage;
        public UIntPtr QuotaNonPagedPoolUsage;
        public UIntPtr PagefileUsage;
        public UIntPtr PeakPagefileUsage;
        public UIntPtr PrivateUsage;
        public UIntPtr PrivateWorkingSetSize;
        public ulong SharedCommitUsage;
    }

    [DllImport("psapi.dll", SetLastError = true)]
    static extern bool GetProcessMemoryInfo(IntPtr h, ref PMC2 c, uint cb);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll")]
    static extern bool GetProcessTimes(IntPtr h, out long c, out long e, out long k, out long u);
    [DllImport("kernel32.dll")]
    static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")]
    static extern bool GetSystemTimes(out long idle, out long kernel, out long user);
    [DllImport("user32.dll")]
    public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")]
    public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")]
    public static extern IntPtr GetThreadDpiAwarenessContext();
    [DllImport("user32.dll")]
    public static extern int GetAwarenessFromDpiAwarenessContext(IntPtr v);

    static Dictionary<int, IntPtr> H = new Dictionary<int, IntPtr>();

    public static void Reset()
    {
        foreach (var h in H.Values) CloseHandle(h);
        H.Clear();
    }

    public static int[] Pids()
    {
        var r = new int[H.Count]; H.Keys.CopyTo(r, 0); return r;
    }

    // Opens and keeps a handle so CPU time stays readable after the process exits
    // and the PID cannot be reused while we track it.
    public static bool Track(int pid)
    {
        if (H.ContainsKey(pid)) return true;
        IntPtr h = OpenProcess(0x1000 /*QUERY_LIMITED*/ | 0x0010 /*VM_READ*/, false, pid);
        if (h == IntPtr.Zero) h = OpenProcess(0x1000, false, pid);
        if (h == IntPtr.Zero) return false;
        H[pid] = h;
        return true;
    }

    // [privateUsage, workingSet, privateWorkingSet, cpu100ns, exited(0/1), creationFileTime]
    public static long[] Query(int pid)
    {
        IntPtr h;
        if (!H.TryGetValue(pid, out h)) { if (!Track(pid)) return null; h = H[pid]; }
        var r = new long[6];
        for (int i = 0; i < 6; i++) r[i] = -1;
        uint code;
        bool exited = GetExitCodeProcess(h, out code) && code != 259;
        r[4] = exited ? 1 : 0;
        if (!exited)
        {
            var c = new PMC2();
            c.cb = (uint)Marshal.SizeOf(typeof(PMC2));
            if (GetProcessMemoryInfo(h, ref c, c.cb))
            {
                r[0] = (long)c.PrivateUsage.ToUInt64();
                r[1] = (long)c.WorkingSetSize.ToUInt64();
                r[2] = (long)c.PrivateWorkingSetSize.ToUInt64();
            }
        }
        long cr, e, k, u;
        if (GetProcessTimes(h, out cr, out e, out k, out u)) { r[3] = k + u; r[5] = cr; }
        return r;
    }

    public static long[] SysTimes()
    {
        long i, k, u; GetSystemTimes(out i, out k, out u);
        return new long[] { i, k, u };
    }
}
