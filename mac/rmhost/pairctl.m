/*
 * Pairing on a running host, controlled through standard input.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Commands, one per line:
 *   pair open SECONDS   let the next unknown device ask to be let in, for at most SECONDS
 *   pair accept FP      trust the device that is waiting, if its fingerprint is FP
 *   pair decline FP     turn the device that is waiting away, if its fingerprint is FP
 *   pair close          stop pairing; a device that is waiting is declined
 *
 * Lines printed, in this fixed form, for whoever runs rmhost to read:
 *   rmhost: pairing open until UNIX_SECONDS     (0 for -p, which has no time limit)
 *   rmhost: pairing pending JSON                (a device waits; JSON is the core's own)
 *   rmhost: paired FINGERPRINT NAME             (NAME is the rest of the line)
 *   rmhost: another device is waiting now       (pair accept or decline named another one)
 *   rmhost: pairing closed: paired|declined|timeout|asked|other
 *
 * The JSON of `pending` is what rm_host_pending gives and what remoteman's own host shows as
 * `status.pending`: {"name":"...","fingerprint":"...","code":"123 456","seconds":60}, with
 * `seconds` what the owner still has. The code is made from both keys; the device shows the
 * same one, for the person to compare before accepting.
 *
 * Pairing opened by `pair open` asks first: remoteman's core is put in confirm mode, so a
 * device it does not know is held until `pair accept` or `pair decline`, or refused after 60
 * seconds. Only `-p` on the command line, which is for trying it by hand, lets the next device in
 * with no question. The core turns pairing off the moment an unknown device connects, so a
 * waiting device uses up that pairing whatever the answer. The time limit therefore counts
 * only while nobody waits, and pairing is never closed under a waiting device by time.
 *
 * remoteman's core turns pairing off by itself once one device has paired, so pairing is never
 * open for more than one device, and rmhost turns it off when the time is up, so an agent that
 * crashes cannot leave it open. The core has no callback for "a device paired", so the
 * trusted devices are looked at every tick.
 *
 * What the core does when an unknown device connects (host.rs, accept_loop): it turns
 * pairing off first, then adds the device under its IP address, and renames it to the name in
 * the device's hello a little later. So a look can find pairing off with no new device yet,
 * and a new device that still has its IP address as its name. A pairing is therefore decided
 * from the device list changing: a closed pairing waits a moment for the device to show up,
 * and a new device is named only once it has its real name, or after a short wait.
 *
 * End of standard input is not an end of rmhost: a host started from a terminal or a
 * script, with input closed, keeps running as before.
 */
#import <Foundation/Foundation.h>
#include <string.h>
#include <arpa/inet.h>
#include <time.h>
#include <unistd.h>

#include "devicectl.h"
#include "pairctl.h"

#define TICK_MS 200
#define MAX_SECONDS 3600
#define DEVICES_BUFFER 16384
/* How long, after pairing went off, to wait for the device to be added; and how long to wait
 * for a new device's own name before printing its address instead. */
#define APPEAR_WAIT_S 1.5
#define NAME_WAIT_S 2.0
/* The core refuses a held device after this long; a wait that ends near it was a time-out. */
#define CONFIRM_WAIT_S 60.0
#define TIMEOUT_SLACK_S 3.0

typedef struct pairctl {
    rm_host *host;
    dispatch_queue_t queue;
    dispatch_source_t input, tick;
    NSMutableData *pending;      /* a partial line from standard input */
    NSSet<NSString *> *known;    /* fingerprints trusted when pairing was opened */
    bool open;
    time_t deadline;             /* 0: no limit (-p) */
    double off_since;            /* when pairing was first seen off with no new device, or 0 */
    double new_since;            /* when a new device was first seen, or 0 */
    NSString *waiting_fp;        /* the device that waits for an answer, or nil */
    double waiting_since;
    int decided;                 /* what was said to it: 0 nothing, 1 accept, 2 decline */
    const char *why;             /* why the pairing ends if no device paired, or NULL */
} pairctl;

static pairctl ctl;

/* The trusted devices, as fingerprint to name, in the order they were paired. */
static NSArray<NSDictionary *> *devices(void)
{
    char *buf = malloc(DEVICES_BUFFER);
    size_t len = rm_host_devices(ctl.host, buf, DEVICES_BUFFER);
    if (len >= DEVICES_BUFFER) {
        free(buf);
        buf = malloc(len + 1);
        rm_host_devices(ctl.host, buf, len + 1);
    }
    NSData *json = [NSData dataWithBytes:buf length:strnlen(buf, len)];
    free(buf);
    id list = [NSJSONSerialization JSONObjectWithData:json options:0 error:nil];
    return [list isKindOfClass:NSArray.class] ? list : @[];
}

static NSSet<NSString *> *fingerprints(void)
{
    NSMutableSet *set = [NSMutableSet set];
    for (NSDictionary *device in devices()) {
        NSString *fp = device[@"fingerprint"];
        if (fp) {
            [set addObject:fp];
        }
    }
    return set;
}

static double now_s(void)
{
    return [NSDate timeIntervalSinceReferenceDate];
}

/* Whether a name is an address: what the core calls a device until its hello names it. */
static bool is_address(NSString *name)
{
    unsigned char buf[16];
    return inet_pton(AF_INET, name.UTF8String, buf) == 1 ||
           inet_pton(AF_INET6, name.UTF8String, buf) == 1;
}

/* The devices that paired since pairing opened. */
static NSArray<NSDictionary *> *new_devices(void)
{
    NSMutableArray *list = [NSMutableArray array];
    for (NSDictionary *device in devices()) {
        NSString *fp = device[@"fingerprint"];
        if (fp && ![ctl.known containsObject:fp]) {
            [list addObject:device];
        }
    }
    return list;
}

/* The device waiting for an answer, as the core gives it; nil when none waits. Asking also
 * tells the core that somebody is there to answer. */
static NSDictionary *waiting(NSString **json_out)
{
    char buf[1024];
    size_t len = rm_host_pending(ctl.host, buf, sizeof(buf));
    if (len == 0 || len >= sizeof(buf)) {
        return nil;
    }
    NSData *json = [NSData dataWithBytes:buf length:len];
    id device = [NSJSONSerialization JSONObjectWithData:json options:0 error:nil];
    if (![device isKindOfClass:NSDictionary.class]) {
        return nil;
    }
    *json_out = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
    return device;
}

static void say(NSString *line)
{
    printf("%s\n", line.UTF8String);
    fflush(stdout);
}

static void begin(time_t deadline)
{
    if (!ctl.open) {
        ctl.known = fingerprints();
    }
    ctl.open = true;
    ctl.deadline = deadline;
    ctl.off_since = ctl.new_since = 0;
    ctl.waiting_fp = nil;
    ctl.decided = 0;
    ctl.why = NULL;
    /* Ask first, whoever comes: the app never lets a device in unseen. */
    rm_host_set_confirm(ctl.host, 1);
    rm_host_set_pairing(ctl.host, 1);
    say([NSString stringWithFormat:@"rmhost: pairing open until %ld", (long)deadline]);
}

/* Closes pairing and says why. Devices that paired since it opened are named first, and a
 * pairing is always closed as "paired", whatever else was the cause. */
static void finish(const char *why)
{
    bool paired = false;
    for (NSDictionary *device in new_devices()) {
        devicectl_new_device(device[@"fingerprint"]);
        say([NSString stringWithFormat:@"rmhost: paired %@ %@", device[@"fingerprint"],
                                       device[@"name"] ?: @""]);
        paired = true;
    }
    /* Closing pairing declines a device that is still waiting. */
    rm_host_decide(ctl.host, 0);
    rm_host_set_pairing(ctl.host, 0);
    rm_host_set_confirm(ctl.host, 0);
    ctl.open = false;
    ctl.waiting_fp = nil;
    say([NSString stringWithFormat:@"rmhost: pairing closed: %s", paired ? "paired" : why]);
}

static void command(NSString *line)
{
    NSArray<NSString *> *words = [line componentsSeparatedByCharactersInSet:
                                           NSCharacterSet.whitespaceCharacterSet];
    if (words.count == 3 && [words[0] isEqual:@"pair"] && [words[1] isEqual:@"open"]) {
        long seconds = words[2].integerValue;
        if (seconds < 1 || seconds > MAX_SECONDS) {
            say(@"rmhost: pair open needs seconds from 1 to 3600");
            return;
        }
        begin(time(NULL) + seconds);
    } else if (words.count == 3 && [words[0] isEqual:@"pair"] &&
               ([words[1] isEqual:@"accept"] || [words[1] isEqual:@"decline"])) {
        /* The device is named by the fingerprint the app showed, and read again here, straight
         * before the answer: the answer goes only to that device. */
        bool accept = [words[1] isEqual:@"accept"];
        NSString *json = nil;
        NSDictionary *device = waiting(&json);
        NSString *fp = device[@"fingerprint"];
        if (!device) {
            say(@"rmhost: nobody is waiting");
        } else if (![fp isKindOfClass:NSString.class] ||
                   ![fp.lowercaseString isEqualToString:words[2].lowercaseString]) {
            say(@"rmhost: another device is waiting now");
        } else if (rm_host_decide(ctl.host, accept)) {
            ctl.decided = accept ? 1 : 2;
        } else {
            say(@"rmhost: nobody is waiting");
        }
    } else if (words.count == 2 && [words[0] isEqual:@"pair"] && [words[1] isEqual:@"close"]) {
        if (ctl.open) {
            finish("asked");
        }
    } else if (line.length && !devicectl_command(words)) {
        say([NSString stringWithFormat:@"rmhost: unknown command: %@", line]);
    }
}

/* Every tick: did a device pair, has the core closed pairing, is the time up? */
static void look(void)
{
    devicectl_tick();
    if (!ctl.open) {
        return;
    }
    double now = now_s();
    NSString *json = nil;
    NSDictionary *device = waiting(&json);
    if (device) {
        /* Nobody is let in or turned away by time while a device waits for its answer. */
        NSString *fp = device[@"fingerprint"];
        if (![fp isEqual:ctl.waiting_fp]) {
            ctl.waiting_fp = fp;
            ctl.waiting_since = now;
            ctl.decided = 0;
            ctl.off_since = 0;
            say([NSString stringWithFormat:@"rmhost: pairing pending %@", json]);
        }
        return;
    }
    if (ctl.waiting_fp) {
        /* It waited and no longer does: answered, refused after the minute, or gone. */
        ctl.waiting_fp = nil;
        ctl.why = ctl.decided == 2 ? "declined"
                  : now - ctl.waiting_since >= CONFIRM_WAIT_S - TIMEOUT_SLACK_S ? "timeout"
                                                                                 : "other";
        ctl.off_since = now;
    }
    NSArray<NSDictionary *> *fresh = new_devices();
    if (fresh.count) {
        /* A device joined. Wait for its own name, but not for long. */
        if (!ctl.new_since) {
            ctl.new_since = now;
        }
        bool named = true;
        for (NSDictionary *device in fresh) {
            named = named && !is_address(device[@"name"] ?: @"");
        }
        if (named || now - ctl.new_since >= NAME_WAIT_S) {
            finish("paired");
        }
        return;
    }
    if (!rm_host_pairing(ctl.host)) {
        /* Off with no device yet: the core turns it off just before it adds one. */
        if (!ctl.off_since) {
            ctl.off_since = now;
        } else if (now - ctl.off_since >= APPEAR_WAIT_S) {
            finish(ctl.why ?: "other");
        }
        return;
    }
    if (ctl.deadline && time(NULL) >= ctl.deadline) {
        finish("timeout");
    }
}

static void read_input(void)
{
    char buf[512];
    ssize_t got = read(STDIN_FILENO, buf, sizeof(buf));
    if (got <= 0) {
        /* End of input: stop reading, keep serving. */
        dispatch_source_cancel(ctl.input);
        return;
    }
    [ctl.pending appendBytes:buf length:got];
    for (;;) {
        const char *bytes = ctl.pending.bytes;
        const char *newline = memchr(bytes, '\n', ctl.pending.length);
        if (!newline) {
            break;
        }
        NSString *line = [[NSString alloc] initWithBytes:bytes length:newline - bytes
                                                encoding:NSUTF8StringEncoding];
        [ctl.pending replaceBytesInRange:NSMakeRange(0, newline - bytes + 1) withBytes:NULL length:0];
        command([line stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]);
    }
}

void pairctl_start(rm_host *host, int already_open)
{
    ctl.host = host;
    devicectl_start(host);
    ctl.pending = [NSMutableData data];
    ctl.queue = dispatch_queue_create("rmhost.pairing", DISPATCH_QUEUE_SERIAL);
    ctl.known = [NSSet set];
    if (already_open) {
        /* -p: open from the start, with no time limit, until a device pairs. */
        ctl.known = fingerprints();
        ctl.open = true;
        say(@"rmhost: pairing open until 0");
    }
    ctl.input = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, STDIN_FILENO, 0, ctl.queue);
    dispatch_source_set_event_handler(ctl.input, ^{
        read_input();
    });
    dispatch_resume(ctl.input);
    ctl.tick = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, ctl.queue);
    dispatch_source_set_timer(ctl.tick, DISPATCH_TIME_NOW, TICK_MS * NSEC_PER_MSEC, 0);
    dispatch_source_set_event_handler(ctl.tick, ^{
        look();
    });
    dispatch_resume(ctl.tick);
}
