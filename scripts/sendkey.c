// sendkey.c — 通过 /dev/uinput 发送组合键给 Titan
// gcc -o sendkey sendkey.c && sudo ./sendkey <key>
// key: super_d, super_grave, super_p
#include <fcntl.h>
#include <linux/uinput.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>

void emit(int fd, int type, int code, int val) {
    struct input_event ev = {0};
    ev.type = type;
    ev.code = code;
    ev.value = val;
    write(fd, &ev, sizeof(ev));
    usleep(1000);
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "Usage: %s <key:super_d|super_grave|super_p|super_space>\n", argv[0]); return 1; }

    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) { perror("open /dev/uinput"); return 1; }

    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    // 注册常用键
    ioctl(fd, UI_SET_KEYBIT, KEY_LEFTMETA);
    ioctl(fd, UI_SET_KEYBIT, KEY_D);
    ioctl(fd, UI_SET_KEYBIT, KEY_GRAVE);
    ioctl(fd, UI_SET_KEYBIT, KEY_P);
    ioctl(fd, UI_SET_KEYBIT, KEY_SPACE);

    struct uinput_setup setup = {0};
    setup.id.bustype = BUS_USB;
    setup.id.vendor = 0x1234;
    setup.id.product = 0x5678;
    strncpy(setup.name, "ydotool virtual device", UINPUT_MAX_NAME_SIZE);
    ioctl(fd, UI_DEV_SETUP, &setup);
    ioctl(fd, UI_DEV_CREATE);
    sleep(1); // 等待 libinput 发现新设备

    int key_code;
    if (strcmp(argv[1], "super_d") == 0) key_code = KEY_D;
    else if (strcmp(argv[1], "super_grave") == 0) key_code = KEY_GRAVE;
    else if (strcmp(argv[1], "super_p") == 0) key_code = KEY_P;
    else if (strcmp(argv[1], "super_space") == 0) key_code = KEY_SPACE;
    else { fprintf(stderr, "Unknown key: %s\n", argv[1]); return 1; }

    printf("Sending Super+%s (key_code=%d)\n", argv[1], key_code);

    // Super down
    emit(fd, EV_KEY, KEY_LEFTMETA, 1);
    emit(fd, EV_SYN, SYN_REPORT, 0);
    usleep(50000);
    // Key down
    emit(fd, EV_KEY, key_code, 1);
    emit(fd, EV_SYN, SYN_REPORT, 0);
    usleep(50000);
    // Key up
    emit(fd, EV_KEY, key_code, 0);
    emit(fd, EV_SYN, SYN_REPORT, 0);
    usleep(50000);
    // Super up
    emit(fd, EV_KEY, KEY_LEFTMETA, 0);
    emit(fd, EV_SYN, SYN_REPORT, 0);

    usleep(100000);
    ioctl(fd, UI_DEV_DESTROY);
    close(fd);
    printf("Done\n");
    return 0;
}
