// Standalone FPX login host for Aniimo. Runs next to FPX.dll under Proton.
// Best-effort auto-close after a successful login (callback + child-window heuristics).
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static FILE* lg;
static volatile int g_done;
static volatile int g_login_ok;
static HWND g_host;
#define LOG(...) do { if (lg) { fprintf(lg, __VA_ARGS__); fflush(lg); } } while (0)

static int looks_like(const char* s, const char* needle) {
    return s && needle && strstr(s, needle) != NULL;
}

static void maybe_success(const char* s) {
    if (!s) return;
    if (looks_like(s, "LoginSuccess") || looks_like(s, "loginSuccess") ||
        looks_like(s, "OnLoginSuccess") || looks_like(s, "SDKLoginSuccess") ||
        looks_like(s, "authSuccess") || looks_like(s, "\"login_success\"") ||
        looks_like(s, "\"code\":0") || looks_like(s, "access_token") ||
        looks_like(s, "fp_uid") || looks_like(s, "\"uid\":\"")) {
        LOG("  >> login success signal in callback\n");
        g_login_ok = 1;
        g_done = 1;
    }
}

static void dump_str(const char* tag, void* p) {
    if (!p || IsBadReadPtr(p, 4)) {
        LOG("  %s=%p\n", tag, p);
        return;
    }
    char s[201];
    int j = 0;
    unsigned char* b = (unsigned char*)p;
    for (int i = 0; i < 200 && !IsBadReadPtr(b + i, 1); i++) {
        unsigned char c = b[i];
        if (c == 0 && i > 0) break;
        s[j++] = (c >= 32 && c < 127) ? (char)c : '.';
    }
    s[j] = 0;
    LOG("  %s=%p str=\"%s\"\n", tag, p, s);
    maybe_success(s);
}

/* FPX stores this and invokes it with SDK events; exact arity varies by build. */
static long long __stdcall cb(void* a, void* b, void* c, void* d, void* e) {
    LOG("  >>> CALLBACK a=%p b=%p c=%p d=%p e=%p\n", a, b, c, d, e);
    dump_str("a", a);
    dump_str("b", b);
    return 0;
}

static BOOL CALLBACK count_child(HWND w, LPARAM lp) {
    if (IsWindowVisible(w)) (*(int*)lp)++;
    return TRUE;
}

static int visible_children(HWND host) {
    int n = 0;
    EnumChildWindows(host, count_child, (LPARAM)&n);
    return n;
}

static BOOL CALLBACK count_process_wnd(HWND w, LPARAM lp) {
    DWORD pid = 0;
    GetWindowThreadProcessId(w, &pid);
    if (pid != GetCurrentProcessId()) return TRUE;
    if (w == g_host) return TRUE;
    if (!IsWindowVisible(w)) return TRUE;
    char title[128];
    GetWindowTextA(w, title, sizeof title);
    // Ignore Wine's generic frame that mirrors our process name.
    if (title[0] && _stricmp(title, "fpx_login") == 0) return TRUE;
    (*(int*)lp)++;
    LOG("  extra window hwnd=%p title=\"%s\"\n", (void*)w, title);
    return TRUE;
}

static int extra_windows(void) {
    int n = 0;
    EnumWindows(count_process_wnd, (LPARAM)&n);
    return n;
}

static LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM w, LPARAM l) {
    if (msg == WM_DESTROY) {
        g_done = 1;
        PostQuitMessage(0);
        return 0;
    }
    if (msg == WM_CLOSE) {
        DestroyWindow(hwnd);
        return 0;
    }
    if (msg == WM_ERASEBKGND) {
        RECT r;
        GetClientRect(hwnd, &r);
        FillRect((HDC)w, &r, (HBRUSH)GetStockObject(BLACK_BRUSH));
        return 1;
    }
    return DefWindowProcA(hwnd, msg, w, l);
}

typedef void (*fn1)(void*);
typedef void (*fn2)(void*, void*);

static char* load_config(void) {
    FILE* cf = fopen("config.json", "rb");
    if (!cf) {
        LOG("config.json missing\n");
        return NULL;
    }
    fseek(cf, 0, SEEK_END);
    long n = ftell(cf);
    fseek(cf, 0, SEEK_SET);
    char* cfg = (char*)malloc((size_t)n + 1);
    if (!cfg) { fclose(cf); return NULL; }
    fread(cfg, 1, (size_t)n, cf);
    cfg[n] = 0;
    fclose(cf);
    LOG("config len=%ld\n", n);
    return cfg;
}

int WINAPI WinMain(HINSTANCE hi, HINSTANCE hp, LPSTR cmd, int show) {
    (void)hp; (void)cmd; (void)show;
    lg = fopen("fpx_login.log", "w");
    LOG("fpx_login host start\n");

    WNDCLASSA wc = {0};
    wc.lpfnWndProc = WndProc;
    wc.hInstance = hi;
    wc.lpszClassName = "AniimoFpxHost";
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)GetStockObject(BLACK_BRUSH);
    RegisterClassA(&wc);

    // FunPlus's login card is ~484×459; keep the host tight so there's no black border.
    const int client_w = 484, client_h = 459;
    DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
    RECT wr = {0, 0, client_w, client_h};
    AdjustWindowRect(&wr, style, FALSE);
    int ww = wr.right - wr.left, wh = wr.bottom - wr.top;
    int wx = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
    int wy = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
    if (wx < 0) wx = 0;
    if (wy < 0) wy = 0;
    g_host = CreateWindowExA(0, "AniimoFpxHost", "Aniimo Login",
        style, wx, wy, ww, wh, NULL, NULL, hi, NULL);
    ShowWindow(g_host, SW_SHOW);
    UpdateWindow(g_host);
    LOG("host hwnd=%p size=%dx%d (client %dx%d)\n", (void*)g_host, ww, wh, client_w, client_h);

    char* cfg = load_config();
    if (!cfg) {
        MessageBoxA(NULL, "config.json missing next to fpx_login.exe", "Aniimo Login", MB_OK | MB_ICONERROR);
        return 1;
    }

    HMODULE h = LoadLibraryA("FPX.dll");
    LOG("LoadLibrary FPX.dll=%p err=%lu\n", (void*)h, GetLastError());
    if (!h) {
        MessageBoxA(NULL, "Could not load FPX.dll.\nRun from Aniimo_Data\\Plugins\\x86_64.",
                    "Aniimo Login", MB_OK | MB_ICONERROR);
        return 1;
    }

    fn1 InitCb = (fn1)GetProcAddress(h, "FPX_InitCallback");
    fn2 CoreInit = (fn2)GetProcAddress(h, "FPXCORE_Init");
    fn2 Init = (fn2)GetProcAddress(h, "FPX_Init");
    fn1 Login = (fn1)GetProcAddress(h, "FPX_Login");
    LOG("procs InitCb=%p CoreInit=%p Init=%p Login=%p\n", InitCb, CoreInit, Init, Login);

    if (InitCb) InitCb((void*)cb);
    if (CoreInit) CoreInit((void*)0x1, (void*)cfg);
    if (Init) Init((void*)g_host, (void*)cfg);
    LOG("init done\n");

    MSG msg;
    for (int i = 0; i < 40; i++) {
        while (PeekMessage(&msg, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessage(&msg);
        }
        Sleep(25);
    }
    LOG("FPX_Login\n");
    if (Login) Login((void*)0);

    int peak_children = visible_children(g_host);
    int peak_extra = extra_windows();
    int saw_ui = 0;
    DWORD gone_since = 0;
    LOG("pumping (children=%d extra=%d); will auto-close after login if we can detect it\n",
        peak_children, peak_extra);

    DWORD start = GetTickCount();
    while (!g_done) {
        while (PeekMessage(&msg, NULL, 0, 0, PM_REMOVE)) {
            if (msg.message == WM_QUIT) g_done = 1;
            TranslateMessage(&msg);
            DispatchMessage(&msg);
        }

        int ch = visible_children(g_host);
        int ex = extra_windows();
        if (ch > peak_children) peak_children = ch;
        if (ex > peak_extra) peak_extra = ex;
        if (ch > 0 || ex > 0) {
            saw_ui = 1;
            gone_since = 0;
        } else if (saw_ui) {
            if (!gone_since) gone_since = GetTickCount();
            // Login panel closed after being shown — treat as done (success or cancel).
            if (GetTickCount() - gone_since > 1500) {
                LOG("login UI closed (children peak=%d extra peak=%d); exiting\n",
                    peak_children, peak_extra);
                g_done = 1;
                break;
            }
        }

        if (GetTickCount() - start > 600000) {
            LOG("timeout\n");
            break;
        }
        Sleep(50);
    }

    if (g_login_ok) {
        // Brief pause so FPX can flush session files into the prefix.
        Sleep(800);
    }
    LOG("exit ok=%d\n", g_login_ok);
    if (lg) fclose(lg);
    return 0;
}
