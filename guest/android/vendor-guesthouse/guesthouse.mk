# What Guesthouse adds to Android for VMs. Inherited by the virtio_x86_64 product.

PRODUCT_PACKAGES += \
    GuesthouseSetup \
    GmsCore \
    GuesthouseMicrogCompanion

PRODUCT_COPY_FILES += \
    vendor/guesthouse/setup/privapp-permissions-guesthouse.xml:$(TARGET_COPY_OUT_SYSTEM_EXT)/etc/permissions/privapp-permissions-guesthouse.xml

# ARM64 apps through Digitalis (Apache-2.0), an open ARM64 backend for AOSP's Berberis.
$(call inherit-product-if-exists, vendor/digitalis/prebuilts/digitalis-prebuilts.mk)
