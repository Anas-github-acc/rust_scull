# SPDX-License-Identifier: GPL-2.0

KDIR ?= /lib/modules/`uname -r`/build

MODULE_NAME = rust_ko

all:
	$(MAKE) -C $(KDIR) M=$$PWD modules

clean:
	$(MAKE) -C $(KDIR) M=$$PWD clean

modules_install: default
	$(MAKE) -C $(KDIR) M=$$PWD modules_install

.PHONY: all clean modules_install
