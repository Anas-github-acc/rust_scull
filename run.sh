#!/bin/bash
# run.sh

KERNEL_IMG=../linux/arch/x86/boot/bzImage
ROOTFS_IMG=../buildroot/output/images/rootfs.ext2

DRIVER_DIR=$(pwd)

echo "Starting QEMU VM..."

qemu-system-x86_64 \
    -kernel "$KERNEL_IMG" \
    -drive file="$ROOTFS_IMG",format=raw,index=0,media=disk \
    -append "root=/dev/sda console=ttyS0" \
    -fsdev local,id=scull_dev,path=/home/anas/myspace/cloned/ldd3/scull,security_model=none \
    -device virtio-9p-pci,fsdev=scull_dev,mount_tag=scullshare \
    -fsdev local,id=share,path="$DRIVER_DIR",security_model=passthrough \
    -device virtio-9p-pci,fsdev=share,mount_tag=host_share \
    -serial stdio \
    -enable-kvm \
    -m 2G # Give it 2GB of RAM

echo "QEMU VM has shut down."
