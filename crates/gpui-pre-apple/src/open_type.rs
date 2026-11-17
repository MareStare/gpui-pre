use core_foundation::{
    array::{
        CFArray, CFArrayAppendValue, CFArrayCreateMutable, CFMutableArrayRef, kCFTypeArrayCallBacks,
    },
    base::{CFRelease, CFType, TCFType, kCFAllocatorDefault},
    dictionary::{
        CFDictionary, CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks,
        kCFTypeDictionaryValueCallBacks,
    },
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
use core_text::font_descriptor::{
    TraitAccessors, kCTFontFamilyNameAttribute, kCTFontItalicTrait, kCTFontSlantTrait,
    kCTFontTraitsAttribute, kCTFontWeightTrait,
};
use core_text::{
    font::{CTFont, cascade_list_for_languages},
    font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCreateWithAttributes, kCTFontCascadeListAttribute,
        kCTFontFeatureSettingsAttribute, kCTFontVariationAttribute,
    },
};
use font_kit::font::Font as FontKitFont;
use gpui::{FontFallbacks, FontFeatures};
use objc2_core_text::{
    kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTFontOpticalSizeAttribute,
};

/// The value of the optical size attribute that makes a font draw the
/// default of its optical size axis at each size.
const DEFAULT_OPTICAL_SIZE: &str = "none";

/// Applies the features and the fallbacks to the font. With
/// `default_optical_size`, the font draws at the default of its optical size
/// axis at each size. Core Text otherwise sets the optical size to the font
/// size, and the text systems of the other platforms do not.
pub fn apply_font_attributes(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
    default_optical_size: bool,
) {
    // SAFETY: Each key is a constant of Core Text. Each array is a new array
    // that its wrapper takes ownership of.
    let mut attributes = unsafe {
        vec![(
            CFString::wrap_under_get_rule(kCTFontFeatureSettingsAttribute),
            CFArray::<CFType>::wrap_under_create_rule(generate_feature_array(features)).as_CFType(),
        )]
    };
    if default_optical_size {
        // SAFETY: The key is a constant of Core Text.
        attributes.push(unsafe {
            (
                CFString::wrap_under_get_rule(
                    std::ptr::from_ref(kCTFontOpticalSizeAttribute).cast(),
                ),
                CFString::from_static_string(DEFAULT_OPTICAL_SIZE).as_CFType(),
            )
        });
    }
    if let Some(fallbacks) = fallbacks
        && !fallbacks.fallback_list().is_empty()
    {
        // SAFETY: As above.
        attributes.push(unsafe {
            (
                CFString::wrap_under_get_rule(kCTFontCascadeListAttribute),
                CFArray::<CFType>::wrap_under_create_rule(generate_fallback_array(fallbacks, font))
                    .as_CFType(),
            )
        });
    }
    let attributes = CFDictionary::from_CFType_pairs(&attributes);
    let descriptor = core_text::font_descriptor::new_from_attributes(&attributes);
    *font = copy_with_descriptor(font, &descriptor);
}

/// The 'wght' tag as a number, which keys the weight axis in Core Text.
pub(crate) const WEIGHT_AXIS: i64 = 0x7767_6874;

/// A copy of a variable font at `weight` on its weight axis.
pub(crate) fn with_weight(font: &FontKitFont, weight: u16) -> FontKitFont {
    let variation = CFDictionary::from_CFType_pairs(&[(
        CFNumber::from(WEIGHT_AXIS),
        CFNumber::from(i32::from(weight)),
    )]);
    // SAFETY: The key is a constant of Core Text.
    let key = unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) };
    let attributes = CFDictionary::from_CFType_pairs(&[(key, variation.as_CFType())]);
    let descriptor = core_text::font_descriptor::new_from_attributes(&attributes);
    copy_with_descriptor(font, &descriptor)
}

/// Whether the font draws the default of its optical size axis at each size.
pub(crate) fn has_default_optical_size(font: &CTFont) -> bool {
    // SAFETY: The key is a constant of Core Text.
    let value = unsafe { objc2_font(font).attribute(kCTFontOpticalSizeAttribute) };
    let default: &objc2_core_foundation::CFString =
        objc2_foundation::ns_string!(DEFAULT_OPTICAL_SIZE).as_ref();
    value
        .as_deref()
        .and_then(|value| value.downcast_ref::<objc2_core_foundation::CFString>())
        .is_some_and(|value| value == default)
}

/// The same Core Text font, with the API of objc2.
pub(crate) fn objc2_font(font: &CTFont) -> &objc2_core_text::CTFont {
    // SAFETY: Both types wrap a `CTFontRef`. The borrow keeps the font alive.
    unsafe { &*font.as_concrete_TypeRef().cast() }
}

/// A copy of the font with the attributes of the descriptor added. The copy
/// merges its variation and its features with those of the font.
fn copy_with_descriptor(font: &FontKitFont, descriptor: &CTFontDescriptor) -> FontKitFont {
    unsafe {
        // font-kit uses the older core-text wrappers. Borrow the same CF objects
        // for the generated API, then transfer its create-rule ownership back.
        let native_font = font.native_font();
        let generated_font = objc2_font(&native_font);
        let descriptor = &*descriptor
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFontDescriptor>();
        let new_font = generated_font.copy_with_attributes(0.0, std::ptr::null(), Some(descriptor));
        let new_font = CTFont::wrap_under_create_rule(
            objc2_core_foundation::CFRetained::into_raw(new_font)
                .as_ptr()
                .cast(),
        );
        font_kit::font::Font::from_native_font(&new_font)
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFMutableArrayRef {
    unsafe {
        let feature_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for (tag, value) in features.tag_value_list() {
            let keys = [
                kCTFontOpenTypeFeatureTag as *const _,
                kCTFontOpenTypeFeatureValue as *const _,
            ];
            let tag = CFString::new(tag);
            let value = CFNumber::from(*value as i32);
            let values = [tag.as_CFTypeRef(), value.as_CFTypeRef()];
            let dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                &keys as *const _ as _,
                &values as *const _ as _,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFArrayAppendValue(feature_array, dict as _);
            CFRelease(dict as _);
        }
        feature_array
    }
}

fn generate_fallback_array(fallbacks: &FontFallbacks, font: &mut FontKitFont) -> CFMutableArrayRef {
    unsafe {
        let symbolic_traits = font.native_font().symbolic_traits();
        let all_traits = font.native_font().all_traits();

        let fallback_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for user_fallback in fallbacks.fallback_list() {
            let name = CFString::from(user_fallback.as_str());

            let traits_keys = [kCTFontWeightTrait, kCTFontSlantTrait];
            let weight_value = CFNumber::from(all_traits.normalized_weight());
            let slant_value = CFNumber::from(if (symbolic_traits & kCTFontItalicTrait) != 0 {
                1.0
            } else {
                0.0
            });
            let traits_values = [weight_value.as_CFTypeRef(), slant_value.as_CFTypeRef()];
            let traits = CFDictionaryCreate(
                kCFAllocatorDefault,
                &traits_keys as *const _ as _,
                &traits_values as *const _ as _,
                traits_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            drop(weight_value);
            drop(slant_value);

            let attr_keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let attr_values = [name.as_CFTypeRef(), traits as _];
            let attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                &attr_keys as *const _ as _,
                &attr_values as *const _ as _,
                attr_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(traits as _);

            let fallback_desc = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs as _);

            CFArrayAppendValue(fallback_array, fallback_desc as _);
            CFRelease(fallback_desc as _);
        }

        append_system_fallbacks(fallback_array, &font.native_font());
        fallback_array
    }
}

fn append_system_fallbacks(fallback_array: CFMutableArrayRef, font: &CTFont) {
    unsafe {
        let preferred_languages: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());

        let default_fallbacks = cascade_list_for_languages(font, &preferred_languages);

        for desc in default_fallbacks
            .iter()
            .filter(|desc| desc.font_path().is_some())
        {
            CFArrayAppendValue(fallback_array, desc.as_concrete_TypeRef() as _);
        }
    }
}
