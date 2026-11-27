/*
 * Devices on a running host, controlled through standard input.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Commands, one per line, in the words of remoteman's own Windows host (`ctl`):
 *   devices                                       print the list now
 *   device FINGERPRINT view-only on|off           on: what the device types is not used
 *   device FINGERPRINT allow KIND on|off          KIND: clipboard files sound mic camera
 *   revoke FINGERPRINT                            stop trusting the device; it is dropped
 *   drop                                          disconnect the viewer; it stays trusted
 *
 * Line printed, when the list changes and on `devices`:
 *   rmhost: devices JSON
 * JSON is what remoteman's `status.devices` is: an array of {"fingerprint","name",
 * "connected","permissions":{"input","clipboard","files","sound","mic","camera"}}, in the
 * order the devices were paired. Failures print `rmhost: device: WHY`.
 *
 * rmhost serves video and input only, so of the permissions only `input` changes what a
 * viewer can do here; the others are kept for the core and for a host that offers more.
 */
#include "devicectl.h"

#define DEVICES_BUFFER 16384

static rm_host *host;
static NSString *last_printed;

static void say(NSString *line)
{
    printf("%s\n", line.UTF8String);
    fflush(stdout);
}

static NSArray<NSDictionary *> *trusted(void)
{
    char *buf = malloc(DEVICES_BUFFER);
    size_t len = rm_host_devices(host, buf, DEVICES_BUFFER);
    if (len >= DEVICES_BUFFER) {
        free(buf);
        buf = malloc(len + 1);
        rm_host_devices(host, buf, len + 1);
    }
    NSData *json = [NSData dataWithBytes:buf length:strnlen(buf, len)];
    free(buf);
    id list = [NSJSONSerialization JSONObjectWithData:json options:0 error:nil];
    return [list isKindOfClass:NSArray.class] ? list : @[];
}

static NSDictionary *permissions_of(NSString *fingerprint)
{
    rm_permissions p = { 0 };
    rm_host_get_permissions(host, fingerprint.UTF8String, &p);
    return @{ @"input": [NSNumber numberWithBool:p.input != 0], @"clipboard": [NSNumber numberWithBool:p.clipboard != 0], @"files": [NSNumber numberWithBool:p.files != 0],
              @"sound": [NSNumber numberWithBool:p.sound != 0], @"mic": [NSNumber numberWithBool:p.mic != 0], @"camera": [NSNumber numberWithBool:p.camera != 0] };
}

static NSString *list_json(void)
{
    NSMutableArray *list = [NSMutableArray array];
    for (NSDictionary *device in trusted()) {
        NSString *fp = device[@"fingerprint"];
        if (!fp) {
            continue;
        }
        [list addObject:@{ @"fingerprint": fp, @"name": device[@"name"] ?: @"",
                           @"connected": device[@"connected"] ?: @NO,
                           @"permissions": permissions_of(fp) }];
    }
    NSData *data = [NSJSONSerialization dataWithJSONObject:list
                                                   options:NSJSONWritingSortedKeys error:nil];
    return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

void devicectl_start(rm_host *h)
{
    host = h;
}

void devicectl_tick(void)
{
    NSString *now = list_json();
    if (![now isEqual:last_printed]) {
        last_printed = now;
        say([@"rmhost: devices " stringByAppendingString:now]);
    }
}

void devicectl_new_device(NSString *fingerprint)
{
    rm_permissions p = { 0 };
    rm_host_get_permissions(host, fingerprint.UTF8String, &p);
    p.clipboard = 0;
    p.files = 0;
    rm_host_set_permissions(host, fingerprint.UTF8String, &p);
}

/* Sets one permission of one device. `kind` is a name from the commands above. */
static bool set_permission(NSString *fingerprint, NSString *kind, bool on)
{
    rm_permissions p = { 0 };
    rm_host_get_permissions(host, fingerprint.UTF8String, &p);
    uint8_t *field = [kind isEqual:@"input"] ? &p.input
                     : [kind isEqual:@"clipboard"] ? &p.clipboard
                     : [kind isEqual:@"files"] ? &p.files
                     : [kind isEqual:@"sound"] ? &p.sound
                     : [kind isEqual:@"mic"] ? &p.mic
                     : [kind isEqual:@"camera"] ? &p.camera : NULL;
    if (!field) {
        say([NSString stringWithFormat:@"rmhost: device: %@ is not a permission", kind]);
        return false;
    }
    *field = on;
    return rm_host_set_permissions(host, fingerprint.UTF8String, &p) == 1;
}

static bool is_trusted(NSString *fingerprint)
{
    for (NSDictionary *device in trusted()) {
        if ([device[@"fingerprint"] isEqual:fingerprint]) {
            return true;
        }
    }
    return false;
}

int devicectl_command(NSArray<NSString *> *w)
{
    if (w.count == 1 && [w[0] isEqual:@"devices"]) {
        last_printed = nil;
        devicectl_tick();
        return 1;
    }
    if (w.count == 1 && [w[0] isEqual:@"drop"]) {
        if (!rm_host_drop_client(host)) {
            say(@"rmhost: device: nobody is connected");
        }
        return 1;
    }
    if (w.count == 2 && [w[0] isEqual:@"revoke"]) {
        if (rm_host_revoke(host, w[1].UTF8String) != 1) {
            say(@"rmhost: device: that device is not trusted");
        }
        return 1;
    }
    if (w.count >= 3 && [w[0] isEqual:@"device"]) {
        NSString *fp = w[1];
        if (!is_trusted(fp)) {
            say(@"rmhost: device: that device is not trusted");
            return 1;
        }
        bool on = [w.lastObject isEqual:@"on"];
        if (w.count == 4 && [w[2] isEqual:@"view-only"] && ([w[3] isEqual:@"on"] || [w[3] isEqual:@"off"])) {
            set_permission(fp, @"input", !on);
            return 1;
        }
        if (w.count == 5 && [w[2] isEqual:@"allow"] && ([w[4] isEqual:@"on"] || [w[4] isEqual:@"off"])) {
            set_permission(fp, w[3], on);
            return 1;
        }
    }
    return 0;
}
