package codexbridge

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/mail"
	"net/url"
	"slices"
	"strings"
	"time"
	"unicode/utf8"
)

type mcpFormSchema struct {
	Type       string                     `json:"type"`
	Properties map[string]json.RawMessage `json:"properties"`
	Required   []string                   `json:"required"`
}

type mcpPrimitiveSchema struct {
	Type        string           `json:"type"`
	Title       string           `json:"title"`
	Description string           `json:"description"`
	Format      string           `json:"format"`
	Enum        []string         `json:"enum"`
	EnumNames   []string         `json:"enumNames"`
	OneOf       []mcpConstOption `json:"oneOf"`
	Items       json.RawMessage  `json:"items"`
	Minimum     *float64         `json:"minimum"`
	Maximum     *float64         `json:"maximum"`
	MinLength   *int             `json:"minLength"`
	MaxLength   *int             `json:"maxLength"`
	MinItems    *int             `json:"minItems"`
	MaxItems    *int             `json:"maxItems"`
}

type mcpConstOption struct {
	Const string `json:"const"`
	Title string `json:"title"`
}

type mcpArrayItems struct {
	Type  string           `json:"type"`
	Enum  []string         `json:"enum"`
	AnyOf []mcpConstOption `json:"anyOf"`
}

var (
	mcpFormKeywords      = []string{"type", "properties", "required", "title", "description"}
	mcpPrimitiveKeywords = []string{"type", "title", "description", "format", "enum", "enumNames", "oneOf", "items", "minimum", "maximum", "minLength", "maxLength", "minItems", "maxItems"}
	mcpOptionKeywords    = []string{"const", "title"}
	mcpArrayKeywords     = []string{"type", "enum", "anyOf"}
)

func validateMCPFormSchema(schemaRaw json.RawMessage) (mcpFormSchema, error) {
	var schema mcpFormSchema
	if err := decodeMCPObject(schemaRaw, &schema, mcpFormKeywords); err != nil {
		return schema, fmt.Errorf("invalid MCP form schema: %w", err)
	}
	if schema.Type != "object" || schema.Properties == nil {
		return schema, errors.New("MCP form schema must be an object with properties")
	}
	for _, required := range schema.Required {
		if _, exists := schema.Properties[required]; !exists {
			return schema, fmt.Errorf("required field %q has no property schema", required)
		}
	}
	for name, raw := range schema.Properties {
		if err := validateMCPPrimitiveSchema(name, raw); err != nil {
			return schema, err
		}
	}
	return schema, nil
}

func validateMCPPrimitiveSchema(name string, raw json.RawMessage) error {
	var schema mcpPrimitiveSchema
	if err := decodeMCPObject(raw, &schema, mcpPrimitiveKeywords); err != nil {
		return fmt.Errorf("field %q has an invalid schema: %w", name, err)
	}
	if err := validateMCPOptions(raw, "oneOf"); err != nil {
		return fmt.Errorf("field %q has invalid oneOf options: %w", name, err)
	}
	if schema.Minimum != nil && schema.Maximum != nil && *schema.Minimum > *schema.Maximum {
		return fmt.Errorf("field %q has minimum greater than maximum", name)
	}
	if err := validNonnegativeRange(name, "length", schema.MinLength, schema.MaxLength); err != nil {
		return err
	}
	if err := validNonnegativeRange(name, "item count", schema.MinItems, schema.MaxItems); err != nil {
		return err
	}
	switch schema.Type {
	case "string":
		if len(schema.Items) != 0 || schema.Minimum != nil || schema.Maximum != nil || schema.MinItems != nil || schema.MaxItems != nil {
			return fmt.Errorf("field %q combines incompatible string constraints", name)
		}
		if len(schema.EnumNames) > 0 && len(schema.EnumNames) != len(schema.Enum) {
			return fmt.Errorf("field %q has enumNames without matching enum values", name)
		}
		if len(schema.Enum) > 0 && len(schema.OneOf) > 0 {
			return fmt.Errorf("field %q combines enum and oneOf", name)
		}
		if !supportedStringFormat(schema.Format) {
			return fmt.Errorf("field %q uses unsupported format %q", name, schema.Format)
		}
	case "number", "integer":
		if schema.Format != "" || len(schema.Enum) > 0 || len(schema.OneOf) > 0 || len(schema.Items) != 0 || schema.MinLength != nil || schema.MaxLength != nil || schema.MinItems != nil || schema.MaxItems != nil {
			return fmt.Errorf("field %q combines incompatible numeric constraints", name)
		}
	case "boolean":
		if schema.Format != "" || len(schema.Enum) > 0 || len(schema.OneOf) > 0 || len(schema.Items) != 0 || schema.Minimum != nil || schema.Maximum != nil || schema.MinLength != nil || schema.MaxLength != nil || schema.MinItems != nil || schema.MaxItems != nil {
			return fmt.Errorf("field %q combines incompatible boolean constraints", name)
		}
	case "array":
		if len(schema.Items) == 0 || schema.Format != "" || len(schema.Enum) > 0 || len(schema.OneOf) > 0 || schema.Minimum != nil || schema.Maximum != nil || schema.MinLength != nil || schema.MaxLength != nil {
			return fmt.Errorf("field %q has invalid array constraints", name)
		}
		var items mcpArrayItems
		if err := decodeMCPObject(schema.Items, &items, mcpArrayKeywords); err != nil {
			return fmt.Errorf("field %q has invalid array constraints: %w", name, err)
		}
		if err := validateMCPOptions(schema.Items, "anyOf"); err != nil {
			return fmt.Errorf("field %q has invalid anyOf options: %w", name, err)
		}
		if items.Type != "string" || (len(items.Enum) == 0 && len(items.AnyOf) == 0) || (len(items.Enum) > 0 && len(items.AnyOf) > 0) {
			return fmt.Errorf("field %q must be an array of enumerated strings", name)
		}
	default:
		return fmt.Errorf("field %q uses unsupported primitive type %q", name, schema.Type)
	}
	return nil
}

func decodeMCPObject(raw json.RawMessage, destination any, allowed []string) error {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(raw, &object); err != nil || object == nil {
		if err != nil {
			return err
		}
		return errors.New("schema must be a JSON object")
	}
	for keyword := range object {
		if !slices.Contains(allowed, keyword) {
			return fmt.Errorf("unsupported schema keyword %q", keyword)
		}
	}
	return json.Unmarshal(raw, destination)
}

func validateMCPOptions(raw json.RawMessage, keyword string) error {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(raw, &object); err != nil {
		return err
	}
	optionsRaw, exists := object[keyword]
	if !exists {
		return nil
	}
	var options []json.RawMessage
	if err := json.Unmarshal(optionsRaw, &options); err != nil {
		return err
	}
	for _, optionRaw := range options {
		var option mcpConstOption
		if err := decodeMCPObject(optionRaw, &option, mcpOptionKeywords); err != nil {
			return err
		}
		var object map[string]json.RawMessage
		_ = json.Unmarshal(optionRaw, &object)
		constRaw, exists := object["const"]
		var value string
		if !exists || json.Unmarshal(constRaw, &value) != nil {
			return errors.New("each option must contain a string const")
		}
	}
	return nil
}

func validNonnegativeRange(name, kind string, minimum, maximum *int) error {
	if minimum != nil && *minimum < 0 || maximum != nil && *maximum < 0 {
		return fmt.Errorf("field %q has a negative %s constraint", name, kind)
	}
	if minimum != nil && maximum != nil && *minimum > *maximum {
		return fmt.Errorf("field %q has minimum %s greater than maximum", name, kind)
	}
	return nil
}

func validateMCPForm(schemaRaw json.RawMessage, contentRaw string) (map[string]any, error) {
	schema, err := validateMCPFormSchema(schemaRaw)
	if err != nil {
		return nil, err
	}
	decoder := json.NewDecoder(strings.NewReader(contentRaw))
	decoder.UseNumber()
	var content map[string]any
	if err := decoder.Decode(&content); err != nil {
		return nil, fmt.Errorf("content must be one JSON object: %w", err)
	}
	if content == nil {
		return nil, errors.New("content must be one JSON object")
	}
	var extra any
	if err := decoder.Decode(&extra); !errors.Is(err, io.EOF) {
		return nil, errors.New("content must contain only one JSON object")
	}
	for _, required := range schema.Required {
		if _, exists := content[required]; !exists {
			return nil, fmt.Errorf("required field %q is missing", required)
		}
	}
	for name, value := range content {
		property, exists := schema.Properties[name]
		if !exists {
			return nil, fmt.Errorf("unknown field %q", name)
		}
		if err := validateMCPPrimitive(name, property, value); err != nil {
			return nil, err
		}
	}
	return content, nil
}

func validateMCPPrimitive(name string, raw json.RawMessage, value any) error {
	var schema mcpPrimitiveSchema
	if err := json.Unmarshal(raw, &schema); err != nil {
		return fmt.Errorf("field %q has an invalid schema", name)
	}
	switch schema.Type {
	case "string":
		text, ok := value.(string)
		if !ok {
			return fmt.Errorf("field %q must be a string", name)
		}
		allowed := schema.Enum
		if len(schema.OneOf) > 0 {
			allowed = make([]string, 0, len(schema.OneOf))
			for _, option := range schema.OneOf {
				allowed = append(allowed, option.Const)
			}
		}
		if len(allowed) > 0 && !slices.Contains(allowed, text) {
			return fmt.Errorf("field %q must be one of %s", name, strings.Join(allowed, ", "))
		}
		length := utf8.RuneCountInString(text)
		if schema.MinLength != nil && length < *schema.MinLength {
			return fmt.Errorf("field %q must contain at least %d characters", name, *schema.MinLength)
		}
		if schema.MaxLength != nil && length > *schema.MaxLength {
			return fmt.Errorf("field %q must contain at most %d characters", name, *schema.MaxLength)
		}
		if err := validateStringFormat(schema.Format, text); err != nil {
			return fmt.Errorf("field %q %w", name, err)
		}
	case "number", "integer":
		number, ok := value.(json.Number)
		if !ok {
			return fmt.Errorf("field %q must be a %s", name, schema.Type)
		}
		parsed, err := number.Float64()
		if err != nil || math.IsInf(parsed, 0) || math.IsNaN(parsed) {
			return fmt.Errorf("field %q must be a finite %s", name, schema.Type)
		}
		if schema.Type == "integer" && math.Trunc(parsed) != parsed {
			return fmt.Errorf("field %q must be an integer", name)
		}
		if schema.Minimum != nil && parsed < *schema.Minimum {
			return fmt.Errorf("field %q must be at least %v", name, *schema.Minimum)
		}
		if schema.Maximum != nil && parsed > *schema.Maximum {
			return fmt.Errorf("field %q must be at most %v", name, *schema.Maximum)
		}
	case "boolean":
		if _, ok := value.(bool); !ok {
			return fmt.Errorf("field %q must be a boolean", name)
		}
	case "array":
		values, ok := value.([]any)
		if !ok {
			return fmt.Errorf("field %q must be an array", name)
		}
		if schema.MinItems != nil && len(values) < *schema.MinItems {
			return fmt.Errorf("field %q must contain at least %d items", name, *schema.MinItems)
		}
		if schema.MaxItems != nil && len(values) > *schema.MaxItems {
			return fmt.Errorf("field %q must contain at most %d items", name, *schema.MaxItems)
		}
		var items mcpArrayItems
		if json.Unmarshal(schema.Items, &items) != nil {
			return fmt.Errorf("field %q has invalid array constraints", name)
		}
		allowed := items.Enum
		if len(items.AnyOf) > 0 {
			for _, option := range items.AnyOf {
				allowed = append(allowed, option.Const)
			}
		}
		for _, value := range values {
			text, ok := value.(string)
			if !ok || !slices.Contains(allowed, text) {
				return fmt.Errorf("field %q contains an invalid option", name)
			}
		}
	default:
		return fmt.Errorf("field %q uses unsupported primitive type %q", name, schema.Type)
	}
	return nil
}

func validateStringFormat(format, value string) error {
	switch format {
	case "":
		return nil
	case "email":
		address, err := mail.ParseAddress(value)
		if err != nil || address.Address != value {
			return errors.New("must be a valid email address")
		}
	case "uri":
		parsed, err := url.ParseRequestURI(value)
		if err != nil || parsed.Scheme == "" {
			return errors.New("must be a valid absolute URI")
		}
	case "date":
		if _, err := time.Parse("2006-01-02", value); err != nil {
			return errors.New("must be an ISO 8601 date")
		}
	case "date-time":
		if _, err := time.Parse(time.RFC3339, value); err != nil {
			return errors.New("must be an RFC 3339 date-time")
		}
	default:
		return fmt.Errorf("uses unsupported format %q", format)
	}
	return nil
}

func supportedStringFormat(format string) bool {
	return format == "" || format == "email" || format == "uri" || format == "date" || format == "date-time"
}

func prettyJSON(raw json.RawMessage) string {
	var output bytes.Buffer
	if json.Indent(&output, raw, "", "  ") != nil {
		return string(raw)
	}
	return output.String()
}
