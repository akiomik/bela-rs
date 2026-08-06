/* Implementation of shim/midi.h. See that file for the contract and
 * ../../docs/midi.md for the decisions behind it.
 *
 * Two rules hold throughout:
 *
 * - No C++ exception may reach Rust, so everything that can allocate is
 *   wrapped. Midi's constructor resizes two vectors, and readFrom and
 *   writeTo build std::strings and a port list.
 * - What a call succeeded at is read back from the object (isInputEnabled,
 *   isOutputEnabled) rather than taken from a return value, because the
 *   return values conflate cases the caller has to tell apart.
 */
#include "midi.h"

#include <cstring>
#include <vector>

#include <libraries/Midi/Midi.h>

namespace {

Midi *self(BelaMidi *midi) { return reinterpret_cast<Midi *>(midi); }

// Runs on the input thread, once per byte of an incoming system
// exclusive message, and does nothing with it.
//
// Not having a callback is not the same as not wanting one: with none
// set, Bela's parser prints every sysex byte with rt_printf, and a
// "Receiving sysex" line around them (Midi.cpp:41). A controller
// announcing itself would fill the program's console. Sysex does not
// reach the message ring either way, so this drops what would
// otherwise be printed.
void discard_sysex(midi_byte_t, void *) {}

} // namespace

extern "C" {

unsigned int bela_midi_list_ports(char *buf, unsigned int len) {
	std::vector<Midi::Port> ports;
	try {
		ports = Midi::listAllPorts();
	} catch (...) {
		return 0;
	}
	unsigned int needed = 0;
	bool copying = true;
	for (const Midi::Port &port : ports) {
		const unsigned int size = port.name.size() + 1;
		// Whole names, and no gaps: a caller reading NUL-terminated
		// strings out of a short buffer must not find a truncated name,
		// nor a later short one written past a longer one that did not
		// fit.
		if (copying && needed + size <= len) {
			memcpy(buf + needed, port.name.c_str(), size);
		} else {
			copying = false;
		}
		needed += size;
	}
	return needed;
}

BelaMidi *bela_midi_new(void) {
	Midi *midi;
	try {
		midi = new Midi();
		// Before any port is open, so no input thread is reading the
		// flag this sets.
		midi->enableParser(true);
		midi->getParser()->setSysexCallback(discard_sysex, nullptr);
	} catch (...) {
		return nullptr;
	}
	return reinterpret_cast<BelaMidi *>(midi);
}

void bela_midi_delete(BelaMidi *midi) {
	// ~Midi joins the input thread; nothing it does throws, but a
	// destructor reached through C is the last place to find out.
	try {
		delete self(midi);
	} catch (...) {
	}
}

int bela_midi_read_from(BelaMidi *midi, const char *port) {
	Midi *m = self(midi);
	int ret;
	try {
		ret = m->readFrom(port);
	} catch (...) {
		return -1;
	}
	if (m->isInputEnabled()) {
		return 0;
	}
	// readFrom answers a port that does not exist with -1, an ALSA
	// failure with -errno, and a thread it could not start with -1.
	// Only the middle one carries anything, so it is passed through and
	// the rest collapse to -1.
	return ret < 0 ? ret : -1;
}

int bela_midi_write_to(BelaMidi *midi, const char *port) {
	Midi *m = self(midi);
	int ret;
	try {
		ret = m->writeTo(port);
	} catch (...) {
		return -1;
	}
	if (m->isOutputEnabled()) {
		return 0;
	}
	// The case this exists for: writeTo returns 1 for a port that does
	// not exist, which is also what it returns on success.
	return ret < 0 ? ret : -1;
}

int bela_midi_available_messages(BelaMidi *midi) {
	MidiParser *parser = self(midi)->getParser();
	if (!parser) {
		return 0;
	}
	return parser->numAvailableMessages();
}

unsigned int bela_midi_get_message(BelaMidi *midi, unsigned char *buf) {
	MidiParser *parser = self(midi)->getParser();
	if (!parser || parser->numAvailableMessages() <= 0) {
		return 0;
	}
	// Returned by value, and advances the read pointer whatever the
	// caller does with it — hence the check above rather than after.
	MidiChannelMessage message = parser->getNextChannelMessage();
	if (message.getType() == kmmNone) {
		return 0;
	}
	buf[0] = message.getStatusByte() | message.getChannel();
	unsigned int size = message.getNumDataBytes();
	// The table it reads maxes out at 2 (Midi.cpp:39). The clamp is so
	// that the buffer contract in the header can be checked here rather
	// than believed about a table in another file.
	if (size > BELA_MIDI_MESSAGE_MAX - 1) {
		size = BELA_MIDI_MESSAGE_MAX - 1;
	}
	for (unsigned int n = 0; n < size; ++n) {
		buf[n + 1] = message.getDataByte(n);
	}
	return size + 1;
}

int bela_midi_write_output(BelaMidi *midi, const unsigned char *bytes,
                           unsigned int length) {
	// const_cast: writeOutput takes a non-const pointer and reads
	// through it (Midi.cpp:512).
	return self(midi)->writeOutput(const_cast<midi_byte_t *>(bytes), length);
}

} /* extern "C" */
