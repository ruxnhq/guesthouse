// SPDX-License-Identifier: GPL-2.0-or-later
package org.guesthouse.setup;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.SystemProperties;
import android.os.UserHandle;
import android.provider.Settings;
import android.util.Log;

/**
 * Applies what Guesthouse chose for this machine. The app passes it in the VM's SMBIOS, GRUB puts it on
 * the kernel's line as androidboot.guesthouse.*, and Android has it as ro.boot.guesthouse.*.
 */
public class BootReceiver extends BroadcastReceiver {
    private static final String TAG = "GuesthouseSetup";
    /** microG's services and its Play Store stand-in, which are on only when the machine has Play services. */
    private static final String[] MICROG = {"com.google.android.gms", "com.android.vending"};
    /** What microG asks for to work as Play services do. Granted when it's turned on, so nothing asks. */
    private static final String[] MICROG_PERMISSIONS = {
        "android.permission.ACCESS_COARSE_LOCATION",
        "android.permission.ACCESS_FINE_LOCATION",
        "android.permission.GET_ACCOUNTS",
        "android.permission.POST_NOTIFICATIONS",
        "android.permission.READ_PHONE_STATE",
    };
    /** Settings.Global's long press of power: 3 shuts down with no question, which is how Guesthouse stops it. */
    private static final String LONG_PRESS = "power_button_long_press";
    private static final int SHUT_DOWN_NO_CONFIRM = 3;

    @Override
    public void onReceive(Context context, Intent intent) {
        boolean play = "1".equals(SystemProperties.get("ro.boot.guesthouse.microg"));
        PackageManager packages = context.getPackageManager();
        for (String name : MICROG) {
            try {
                int wanted = play ? PackageManager.COMPONENT_ENABLED_STATE_ENABLED
                        : PackageManager.COMPONENT_ENABLED_STATE_DISABLED;
                if (packages.getApplicationEnabledSetting(name) != wanted) {
                    packages.setApplicationEnabledSetting(name, wanted, 0);
                }
                if (play && name.equals(MICROG[0])) {
                    for (String permission : MICROG_PERMISSIONS) {
                        try {
                            packages.grantRuntimePermission(name, permission, UserHandle.SYSTEM);
                        } catch (RuntimeException e) {
                            Log.w(TAG, "Could not grant " + permission, e);
                        }
                    }
                }
            } catch (IllegalArgumentException e) {
                Log.i(TAG, name + " isn't in this image");
            }
        }
        Settings.Global.putInt(context.getContentResolver(), LONG_PRESS, SHUT_DOWN_NO_CONFIRM);
        Log.i(TAG, "Play services " + (play ? "on" : "off") + ", long press of power shuts down");
    }
}
