// drm-dump-fb.c — 从 DRM CRTC dump 当前 framebuffer 到 raw 文件
// 用法: sudo ./drm-dump-fb /dev/dri/card1 output.raw
// 编译: gcc -o drm-dump-fb drm-dump-fb.c -ldrm

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

int main(int argc, char *argv[]) {
    if (argc < 3) {
        fprintf(stderr, "Usage: %s /dev/dri/cardN output.raw\n", argv[0]);
        return 1;
    }

    int fd = open(argv[1], O_RDWR);
    if (fd < 0) { perror("open"); return 1; }

    // 找第一个 active CRTC
    drmModeRes *res = drmModeGetResources(fd);
    if (!res) { perror("drmModeGetResources"); return 1; }

    uint32_t fb_id = 0;
    for (int i = 0; i < res->count_crtcs; i++) {
        drmModeCrtc *crtc = drmModeGetCrtc(fd, res->crtcs[i]);
        if (crtc && crtc->buffer_id) {
            fb_id = crtc->buffer_id;
            printf("CRTC %d: fb_id=%u, %ux%u\n", i, fb_id, crtc->width, crtc->height);
            drmModeFreeCrtc(crtc);
            break;
        }
        if (crtc) drmModeFreeCrtc(crtc);
    }
    drmModeFreeResources(res);

    if (!fb_id) {
        fprintf(stderr, "No active CRTC with framebuffer found\n");
        return 1;
    }

    // 获取 FB 信息
    drmModeFBPtr fb = drmModeGetFB(fd, fb_id);
    if (!fb) { perror("drmModeGetFB"); return 1; }

    printf("FB: %ux%u, pitch=%u, bpp=%u, depth=%u, handle=%u\n",
           fb->width, fb->height, fb->pitch, fb->bpp, fb->depth, fb->handle);

    uint32_t width = fb->width;
    uint32_t height = fb->height;
    uint32_t pitch = fb->pitch;

    // 通过 prime handle → fd 导出 GEM buffer
    int prime_fd = -1;
    int ret = drmPrimeHandleToFD(fd, fb->handle, O_RDONLY, &prime_fd);
    if (ret < 0) {
        // fallback: 直接 mmap GEM handle
        struct drm_mode_map_dumb map_req = {};
        map_req.handle = fb->handle;
        ret = drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map_req);
        if (ret < 0) {
            perror("drmPrimeHandleToFD + MAP_DUMB failed");
            // Try direct mmap via lseek
            printf("Trying mmap via prime_fd workaround...\n");
            
            // 最后手段：读 /dev/fb0
            drmModeFreeFB(fb);
            close(fd);
            fprintf(stderr, "Cannot access framebuffer memory. Try reading /dev/fb0 instead.\n");
            return 1;
        }
        
        void *mapped = mmap(0, pitch * height, PROT_READ, MAP_SHARED, fd, map_req.offset);
        if (mapped == MAP_FAILED) { perror("mmap"); return 1; }
        
        FILE *out = fopen(argv[2], "wb");
        fwrite(mapped, 1, pitch * height, out);
        fclose(out);
        munmap(mapped, pitch * height);
        printf("Dumped via dumb mmap: %u bytes\n", pitch * height);
        drmModeFreeFB(fb);
        close(fd);
        return 0;
    }

    // mmap prime fd
    size_t size = (size_t)pitch * height;
    void *mapped = mmap(0, size, PROT_READ, MAP_SHARED, prime_fd, 0);
    if (mapped == MAP_FAILED) { perror("mmap prime"); close(prime_fd); return 1; }

    FILE *out = fopen(argv[2], "wb");
    if (!out) { perror("fopen"); return 1; }
    fwrite(mapped, 1, size, out);
    fclose(out);

    printf("Dumped %zu bytes to %s\n", size, argv[2]);

    munmap(mapped, size);
    close(prime_fd);
    drmModeFreeFB(fb);
    close(fd);
    return 0;
}
