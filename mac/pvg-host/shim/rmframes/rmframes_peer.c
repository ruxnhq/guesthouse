/*
 * rmframes: whether the process at the other end is one of ours.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to
 * deal in the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
 * sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions: the above
 * copyright notice and this permission notice shall be included in all copies
 * or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
 * WITHOUT WARRANTY OF ANY KIND.
 *
 * A bootstrap name is open to every process of the user. A publisher and its
 * readers that are signed by a team take only each other: a peer's code has
 * to meet "anchor apple generic and certificate leaf[subject.OU] = TEAM",
 * with this program's own team, as the kernel's audit token names the peer.
 */
#include "rmframes_peer.h"

#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#include <pthread.h>

static pthread_once_t looked = PTHREAD_ONCE_INIT;
/* Whether this program is signed by a team, and the requirement for its
 * peers then. A team with no requirement made takes no peer. */
static bool has_team;
static SecRequirementRef requirement;

static void look_at_self(void)
{
    SecCodeRef self = NULL;
    SecStaticCodeRef code = NULL;
    CFDictionaryRef information = NULL;
    CFTypeRef team;
    CFStringRef text;

    if (SecCodeCopySelf(kSecCSDefaultFlags, &self) != errSecSuccess ||
        SecCodeCopyStaticCode(self, kSecCSDefaultFlags, &code) != errSecSuccess ||
        SecCodeCopySigningInformation(code, kSecCSSigningInformation,
                                      &information) != errSecSuccess) {
        goto done;
    }
    team = CFDictionaryGetValue(information, kSecCodeInfoTeamIdentifier);
    if (!team || CFGetTypeID(team) != CFStringGetTypeID()) {
        goto done;
    }
    has_team = true;
    text = CFStringCreateWithFormat(
        kCFAllocatorDefault, NULL,
        CFSTR("anchor apple generic and certificate leaf[subject.OU] = \"%@\""),
        team);
    if (text) {
        if (SecRequirementCreateWithString(text, kSecCSDefaultFlags,
                                           &requirement) != errSecSuccess) {
            requirement = NULL;
        }
        CFRelease(text);
    }
done:
    if (information) {
        CFRelease(information);
    }
    if (code) {
        CFRelease(code);
    }
    if (self) {
        CFRelease(self);
    }
}

bool rmf_peer_is_ours(const audit_token_t *token)
{
    CFDataRef data;
    CFDictionaryRef attributes;
    SecCodeRef peer = NULL;
    bool ours = false;

    pthread_once(&looked, look_at_self);
    if (!has_team) {
        return true;
    }
    if (!requirement) {
        return false;
    }
    data = CFDataCreate(kCFAllocatorDefault, (const UInt8 *)token, sizeof(*token));
    if (!data) {
        return false;
    }
    {
        const void *keys[] = { kSecGuestAttributeAudit };
        const void *values[] = { data };

        attributes = CFDictionaryCreate(kCFAllocatorDefault, keys, values, 1,
                                        &kCFTypeDictionaryKeyCallBacks,
                                        &kCFTypeDictionaryValueCallBacks);
    }
    CFRelease(data);
    if (!attributes) {
        return false;
    }
    if (SecCodeCopyGuestWithAttributes(NULL, attributes, kSecCSDefaultFlags,
                                       &peer) == errSecSuccess) {
        ours = SecCodeCheckValidity(peer, kSecCSDefaultFlags, requirement) ==
               errSecSuccess;
        CFRelease(peer);
    }
    CFRelease(attributes);
    return ours;
}
