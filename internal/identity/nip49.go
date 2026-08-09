package identity

import (
	"crypto/rand"
	"errors"
	"fmt"
	"io"

	"github.com/btcsuite/btcd/btcutil/bech32"
	"github.com/wbbradley/hq/internal/event"
	"golang.org/x/crypto/chacha20poly1305"
	"golang.org/x/crypto/scrypt"
	"golang.org/x/text/unicode/norm"
)

const (
	nip49Version        = 2
	NIP49DefaultLogN    = 16
	nip49PayloadLength  = 91
	keySecurityInsecure = 0
)

func EncryptNIP49(secret event.SecretKey, password []byte, logN byte, random io.Reader) (string, error) {
	if logN < 16 || logN > 22 {
		return "", errors.New("NIP-49 log_n must be between 16 and 22")
	}
	if random == nil {
		random = rand.Reader
	}
	salt := make([]byte, 16)
	nonce := make([]byte, chacha20poly1305.NonceSizeX)
	if _, err := io.ReadFull(random, salt); err != nil {
		return "", fmt.Errorf("generate NIP-49 salt: %w", err)
	}
	if _, err := io.ReadFull(random, nonce); err != nil {
		return "", fmt.Errorf("generate NIP-49 nonce: %w", err)
	}
	key, normalized, err := deriveNIP49Key(password, salt, logN)
	if err != nil {
		return "", err
	}
	defer clear(key)
	defer clear(normalized)
	aead, err := chacha20poly1305.NewX(key)
	if err != nil {
		return "", err
	}
	security := []byte{keySecurityInsecure}
	ciphertext := aead.Seal(nil, nonce, secret[:], security)
	payload := make([]byte, 0, nip49PayloadLength)
	payload = append(payload, nip49Version, logN)
	payload = append(payload, salt...)
	payload = append(payload, nonce...)
	payload = append(payload, security...)
	payload = append(payload, ciphertext...)
	return bech32.EncodeFromBase256("ncryptsec", payload)
}

func DecryptNIP49(encoded string, password []byte) (event.SecretKey, error) {
	hrp, words, err := bech32.DecodeNoLimit(encoded)
	if err != nil || hrp != "ncryptsec" {
		return event.SecretKey{}, errors.New("backup is not valid ncryptsec data")
	}
	payload, err := bech32.ConvertBits(words, 5, 8, false)
	if err != nil {
		return event.SecretKey{}, errors.New("backup is not valid ncryptsec data")
	}
	if len(payload) != nip49PayloadLength {
		return event.SecretKey{}, fmt.Errorf("NIP-49 payload is %d bytes; want %d", len(payload), nip49PayloadLength)
	}
	if payload[0] != nip49Version {
		return event.SecretKey{}, fmt.Errorf("unsupported NIP-49 version %d", payload[0])
	}
	logN := payload[1]
	if logN < 16 || logN > 22 {
		return event.SecretKey{}, errors.New("NIP-49 log_n is outside the safe range")
	}
	salt, nonce, security := payload[2:18], payload[18:42], payload[42:43]
	ciphertext := payload[43:]
	key, normalized, err := deriveNIP49Key(password, salt, logN)
	if err != nil {
		return event.SecretKey{}, err
	}
	defer clear(key)
	defer clear(normalized)
	aead, err := chacha20poly1305.NewX(key)
	if err != nil {
		return event.SecretKey{}, err
	}
	plaintext, err := aead.Open(nil, nonce, ciphertext, security)
	if err != nil {
		return event.SecretKey{}, errors.New("password is wrong or backup is corrupt")
	}
	defer clear(plaintext)
	if len(plaintext) != 32 {
		return event.SecretKey{}, errors.New("backup contains an invalid private key")
	}
	return event.SecretKeyFromHex(fmt.Sprintf("%x", plaintext))
}

func deriveNIP49Key(password, salt []byte, logN byte) ([]byte, []byte, error) {
	normalized := norm.NFKC.Bytes(append([]byte(nil), password...))
	key, err := scrypt.Key(normalized, salt, 1<<logN, 8, 1, chacha20poly1305.KeySize)
	if err != nil {
		clear(normalized)
		return nil, nil, fmt.Errorf("derive NIP-49 key: %w", err)
	}
	return key, normalized, nil
}
