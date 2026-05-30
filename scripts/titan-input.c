// titan-input.c — 虚拟键盘/鼠标输入注入工具
// 用法:
//   sudo ./titan-input key <keycode>          — 按下并释放按键
//   sudo ./titan-input keypress <keycode>     — 仅按下
//   sudo ./titan-input keyrelease <keycode>   — 仅释放
//   sudo ./titan-input keycombo <kc1> <kc2> .. — 组合键 (依次按下, 反序释放)
//   sudo ./titan-input mousemove <dx> <dy>    — 移动鼠标
//   sudo ./titan-input mouseclick <button>    — 鼠标点击 (1=左,2=中,3=右)
//   sudo ./titan-input sleep <ms>             — 等待
//   sudo ./titan-input script <file>          — 从文件执行命令序列
// 编译: gcc -o titan-input titan-input.c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <linux/uinput.h>
#include <errno.h>

static int fd = -1;

static void setup() {
    fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) { perror("open /dev/uinput"); exit(1); }

    // 键盘
    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    for (int i = 0; i < 256; i++) ioctl(fd, UI_SET_KEYBIT, i);

    // 鼠标
    ioctl(fd, UI_SET_EVBIT, EV_REL);
    ioctl(fd, UI_SET_RELBIT, REL_X);
    ioctl(fd, UI_SET_RELBIT, REL_Y);

    struct uinput_setup usetup = {0};
    usetup.id.bustype = BUS_USB;
    usetup.id.vendor = 0x1234;
    usetup.id.product = 0x5678;
    strcpy(usetup.name, "Titan Test Input");
    ioctl(fd, UI_DEV_SETUP, &usetup);
    ioctl(fd, UI_DEV_CREATE);
    // 等设备就绪
    usleep(100000);
}

static void emit(int type, int code, int val) {
    struct input_event ie = {0};
    ie.type = type;
    ie.code = code;
    ie.value = val;
    gettimeofday(&ie.time, NULL);
    write(fd, &ie, sizeof(ie));
}

static void do_sync() {
    emit(EV_SYN, SYN_REPORT, 0);
    usleep(1000);
}

static void key_press(int code) { emit(EV_KEY, code, 1); do_sync(); }
static void key_release(int code) { emit(EV_KEY, code, 0); do_sync(); }
static void key_tap(int code) { key_press(code); usleep(50000); key_release(code); }

static void mouse_move(int dx, int dy) {
    emit(EV_REL, REL_X, dx);
    emit(EV_REL, REL_Y, dy);
    do_sync();
}

static void mouse_click(int btn) {
    emit(EV_KEY, btn, 1); do_sync();
    usleep(50000);
    emit(EV_KEY, btn, 0); do_sync();
}

// Linux input keycodes (from input-event-codes.h)
// KEY_ESC=1 KEY_1=2 ... KEY_EQUAL=13 KEY_BACKSPACE=14
// KEY_TAB=15 KEY_Q=16 KEY_W=17 KEY_E=18 KEY_R=19 KEY_T=20
// KEY_LEFTMETA=125 KEY_LEFTCTRL=29 KEY_LEFTSHIFT=42 KEY_LEFTALT=56
// KEY_ENTER=28 KEY_SPACE=57 KEY_A=30 KEY_D=32 ...
// BTN_LEFT=0x110 BTN_MIDDLE=0x111 BTN_RIGHT=0x112

static int parse_keycode(const char *s) {
    // 数字直接用
    int code = atoi(s);
    if (code > 0) return code;

    // 名称映射
    struct { const char *name; int code; } keys[] = {
        {"esc", KEY_ESC}, {"escape", KEY_ESC},
        {"1", KEY_1}, {"2", KEY_2}, {"3", KEY_3}, {"4", KEY_4},
        {"5", KEY_5}, {"6", KEY_6}, {"7", KEY_7}, {"8", KEY_8},
        {"9", KEY_9}, {"0", KEY_0},
        {"minus", KEY_MINUS}, {"equal", KEY_EQUAL},
        {"backspace", KEY_BACKSPACE}, {"tab", KEY_TAB},
        {"q", KEY_Q}, {"w", KEY_W}, {"e", KEY_E}, {"r", KEY_R}, {"t", KEY_T},
        {"y", KEY_Y}, {"u", KEY_U}, {"i", KEY_I}, {"o", KEY_O}, {"p", KEY_P},
        {"a", KEY_A}, {"s", KEY_S}, {"d", KEY_D}, {"f", KEY_F}, {"g", KEY_G},
        {"h", KEY_H}, {"j", KEY_J}, {"k", KEY_K}, {"l", KEY_L},
        {"z", KEY_Z}, {"x", KEY_X}, {"c", KEY_C}, {"v", KEY_V}, {"b", KEY_B},
        {"n", KEY_N}, {"m", KEY_M},
        {"enter", KEY_ENTER}, {"return", KEY_ENTER},
        {"leftshift", KEY_LEFTSHIFT}, {"shift", KEY_LEFTSHIFT},
        {"rightshift", KEY_RIGHTSHIFT},
        {"leftctrl", KEY_LEFTCTRL}, {"ctrl", KEY_LEFTCTRL},
        {"rightctrl", KEY_RIGHTCTRL},
        {"leftalt", KEY_LEFTALT}, {"alt", KEY_LEFTALT},
        {"rightalt", KEY_RIGHTALT},
        {"leftmeta", KEY_LEFTMETA}, {"meta", KEY_LEFTMETA},
        {"super", KEY_LEFTMETA}, {"win", KEY_LEFTMETA},
        {"rightmeta", KEY_RIGHTMETA},
        {"space", KEY_SPACE},
        {"left", KEY_LEFT}, {"right", KEY_RIGHT}, {"up", KEY_UP}, {"down", KEY_DOWN},
        {NULL, 0}
    };
    for (int i = 0; keys[i].name; i++) {
        if (strcasecmp(s, keys[i].name) == 0) return keys[i].code;
    }
    fprintf(stderr, "Unknown key: %s\n", s);
    return -1;
}

static void cleanup() {
    if (fd >= 0) {
        ioctl(fd, UI_DEV_DESTROY);
        close(fd);
    }
}

static int execute_cmd(int argc, char **argv) {
    if (argc < 1) return 0;

    if (strcmp(argv[0], "key") == 0 && argc >= 2) {
        int code = parse_keycode(argv[1]);
        if (code > 0) key_tap(code);
    } else if (strcmp(argv[0], "keypress") == 0 && argc >= 2) {
        int code = parse_keycode(argv[1]);
        if (code > 0) key_press(code);
    } else if (strcmp(argv[0], "keyrelease") == 0 && argc >= 2) {
        int code = parse_keycode(argv[1]);
        if (code > 0) key_release(code);
    } else if (strcmp(argv[0], "keycombo") == 0 && argc >= 3) {
        // 按下所有键
        int codes[16];
        int n = argc - 1;
        if (n > 16) n = 16;
        for (int i = 0; i < n; i++) {
            codes[i] = parse_keycode(argv[i + 1]);
            if (codes[i] > 0) key_press(codes[i]);
        }
        usleep(100000);
        // 反序释放
        for (int i = n - 1; i >= 0; i--) {
            if (codes[i] > 0) key_release(codes[i]);
        }
    } else if (strcmp(argv[0], "mousemove") == 0 && argc >= 3) {
        mouse_move(atoi(argv[1]), atoi(argv[2]));
    } else if (strcmp(argv[0], "mouseclick") == 0 && argc >= 2) {
        int btn = atoi(argv[1]);
        // BTN_LEFT=0x110=272
        mouse_click(btn == 1 ? 0x110 : btn == 2 ? 0x111 : btn == 3 ? 0x112 : btn);
    } else if (strcmp(argv[0], "sleep") == 0 && argc >= 2) {
        usleep(atoi(argv[1]) * 1000);
    } else {
        fprintf(stderr, "Unknown or incomplete command: %s\n", argv[0]);
        return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr,
            "Titan Input Injector\n"
            "Usage: %s <command> [args...]\n"
            "Commands:\n"
            "  key <key>           — 按键\n"
            "  keycombo <k1> <k2>..— 组合键\n"
            "  mousemove <dx> <dy> — 移动鼠标\n"
            "  mouseclick <1|2|3>  — 点击鼠标\n"
            "  sleep <ms>          — 等待\n"
            "  script <file>       — 从文件执行\n"
            "\nKey names: a-z, 0-9, enter, space, esc, tab, backspace,\n"
            "  left/right/up/down, super/win/meta, ctrl, shift, alt\n"
            "  Or raw keycode number\n", argv[0]);
        return 1;
    }

    setup();
    atexit(cleanup);

    if (strcmp(argv[1], "script") == 0 && argc >= 3) {
        // 从文件读取命令序列
        FILE *f = fopen(argv[2], "r");
        if (!f) { perror("fopen"); return 1; }
        char line[256];
        while (fgets(line, sizeof(line), f)) {
            // 跳过注释和空行
            char *p = line;
            while (*p == ' ' || *p == '\t') p++;
            if (*p == '#' || *p == '\n' || *p == '\0') continue;
            // 解析参数
            char *parts[16] = {0};
            int n = 0;
            char *tok = strtok(p, " \t\n");
            while (tok && n < 16) { parts[n++] = tok; tok = strtok(NULL, " \t\n"); }
            if (n > 0) execute_cmd(n, parts);
        }
        fclose(f);
    } else {
        execute_cmd(argc - 1, argv + 1);
    }

    // 等待事件被处理
    usleep(100000);
    return 0;
}
