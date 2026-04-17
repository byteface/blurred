Features:

Core features are in and I'm happy this app does all it needs to. These may be considered nice to haves...

- The 'blurred' state could have a darkmode
- Adjustable text size?
- Inactivity timeout?
- MD file viewer?
- Password to view option?
- Multifile?
- Search?

Bugs:
- Right click showing options we dont need cut/paste. Seems to be an FLTK issue that it can't be overridden.

Todo:
- Test on linux/pc


*Note - be careful of having 'timers' or delayed show/hide. It can cause race conditions as there many ways to toggle hide/show. We've changed all command to a state machine to reduce the risk and wrapped tests around current behaviour