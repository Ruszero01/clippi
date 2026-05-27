// Native ObjC OCR helper — avoids objc2 msg_send! type-encoding issues
#import <Vision/Vision.h>
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>

const char* clippi_ocr_recognize(const char* image_path) {
    @autoreleasepool {
        NSString *path = [NSString stringWithUTF8String:image_path];
        if (!path) {
            return NULL;
        }

        // Load image via NSImage (most reliable cross-format loader)
        NSImage *nsImage = [[NSImage alloc] initWithContentsOfFile:path];
        if (!nsImage || !nsImage.isValid) {
            return NULL;
        }

        // Get CGImage from NSImage
        NSRect rect = NSMakeRect(0, 0, nsImage.size.width, nsImage.size.height);
        CGImageRef cgImage = [nsImage CGImageForProposedRect:&rect context:nil hints:nil];
        if (!cgImage) {
            // Fallback: render via NSBitmapImageRep
            NSData *tiffData = [nsImage TIFFRepresentation];
            if (!tiffData) {
                return NULL;
            }
            NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithData:tiffData];
            if (!rep) {
                return NULL;
            }
            cgImage = [rep CGImage];
        }
        if (!cgImage) {
            return NULL;
        }

        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:cgImage options:@{}];

        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;
        request.recognitionLanguages = @[@"zh-Hans", @"zh-Hant", @"en"];

        NSError *error = nil;
        BOOL success = [handler performRequests:@[request] error:&error];

        if (!success) {
            NSString *msg = error ? error.localizedDescription : @"unknown error";
            NSLog(@"[OCR] performRequests failed: %@", msg);
            return NULL;
        }

        NSArray *results = request.results;
        if (results.count == 0) {
            return strdup("");
        }

        NSMutableString *text = [NSMutableString string];
        for (NSUInteger i = 0; i < results.count; i++) {
            VNRecognizedTextObservation *obs = results[i];
            NSArray<VNRecognizedText *> *candidates = [obs topCandidates:1];
            if (candidates.count > 0) {
                if (text.length > 0) [text appendString:@"\n"];
                [text appendString:candidates.firstObject.string];
            }
        }

        return strdup([text UTF8String]);
    }
}

/// Free the string returned by clippi_ocr_recognize
void clippi_ocr_free_string(const char* s) {
    if (s) free((void*)s);
}
