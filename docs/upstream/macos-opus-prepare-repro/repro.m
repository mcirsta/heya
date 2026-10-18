#import <AVFoundation/AVFoundation.h>
#include <stdio.h>

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 2) {
            fprintf(stderr, "Usage: %s tone.ogg\n", argv[0]);
            return 2;
        }
        NSData *data = [NSData dataWithContentsOfFile:
            [NSString stringWithUTF8String:argv[1]]];
        NSError *error = nil;
        AVAudioPlayer *player = [[AVAudioPlayer alloc] initWithData:data error:&error];
        if (!player) {
            fprintf(stderr, "Initialization failed: %s\n", error.description.UTF8String);
            return 1;
        }
        player.volume = 0;
        printf("Initialized: duration=%.6f, channels=%lu\n", player.duration,
               (unsigned long)player.numberOfChannels);
        puts("Calling prepareToPlay (play is never called)...");
        fflush(stdout);
        BOOL prepared = [player prepareToPlay];
        printf("prepareToPlay returned %s\n", prepared ? "YES" : "NO");
        return prepared ? 0 : 1;
    }
}
