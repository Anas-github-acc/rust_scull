#!/bin/bash
# run.sh

echo 'running the mount process for share file'
mkdir /mnt/share
mount -t 9p -o trans=virtio,version=9p2000.L host_share /mnt/share

mount -t proc proc /proc

# Load your driver!
insmod /mnt/share/rust_ko.ko
